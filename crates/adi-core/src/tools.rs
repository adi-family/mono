//! Renaming a tool — giving one an id a person can read, without breaking the id it had.
//!
//! A tool's id is its directory name under `tools/`, and it is *also* what every agent that can
//! run it writes down: `bin_tools = ["ec5bd98c-35c1-4e9e-ba25-5c2dbd3d5a99", …]`. [`adi_tools`]
//! owns the registry and nothing else, so the move and the definitions that name it are two
//! different stores' work — the same split [`crate::projects`] already makes for a project.
//!
//! The two halves answer different questions, and both are needed:
//!
//! * The registry's **alias** is what stops anything breaking. Every id a tool ever had keeps
//!   resolving, which is the only reason this is safe to do at all against a live store — ids are
//!   cited in generated shims, in a project's `.adi/hive.yaml`, in notes, and in the operator's own
//!   shell history, and none of that is reachable from here.
//! * Re-pointing **`bin_tools`** is what makes the rename worth doing. `bin_tools` is the densest
//!   run of ids in the store and it is read on every launch, so a definition left naming the UUID
//!   still works and still costs exactly what it cost before.
//!
//! **The registry moves first**, for the same reason it does in a project rename: a definition is
//! never re-pointed at an id the move then failed to produce. What follows is reported rather than
//! raised — the tool *has* been renamed by then.

use adi_config::Config;
use adi_tools::{Result, Tool, Tools};

/// What a [`rename_tool`] did — the receipt the CLI prints.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolRenamed {
    /// The tool under its new id.
    pub tool: Tool,
    /// The id it had before, which still resolves to it.
    pub from: String,
    /// Agent definitions whose `bin_tools` now name the new id.
    pub agents: usize,
    /// Stores that could not be carried across, each said in a sentence. Empty on a clean rename;
    /// anything here names a definition still spelling the tool as its old id — which works, and
    /// merely costs what it used to.
    pub warnings: Vec<String>,
}

impl ToolRenamed {
    /// Whether every store followed the rename.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }
}

/// Rename a tool from `from` to `to` across the whole store: the registry first, then every agent
/// definition that has the tool ticked on.
///
/// `from` may be an id the tool no longer has, so a rename can be corrected without first working
/// out which id is current. Renaming to the id it already has is a no-op.
///
/// # Errors
/// [`adi_tools::Error`] from the registry move — an unsafe id, an unregistered `from`, a system
/// tool, an occupied `to`, or an I/O failure. A definition that fails to *follow* the move is
/// reported in [`ToolRenamed::warnings`], not raised.
pub fn rename_tool(config: &Config, from: &str, to: &str) -> Result<ToolRenamed> {
    let tools = Tools::with_config(config.clone());
    // Resolve before the move, so the receipt names the id that was actually vacated rather than
    // whatever alias the caller reached the tool through.
    let was = tools.resolve(from)?;
    let tool = tools.rename(from, to)?;
    let mut report = ToolRenamed {
        tool,
        from: was.clone(),
        agents: 0,
        warnings: Vec::new(),
    };
    if was == to {
        return Ok(report);
    }

    match adi_agents::Agents::with_config(config.clone()).rename_tool(&was, to) {
        Ok(count) => report.agents = count,
        Err(e) => report.warnings.push(format!(
            "agent definitions still name the old id — they could not be rewritten: {e}"
        )),
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-core-rename-tool-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    /// Both halves at once: the definition is rewritten to the cheap id, *and* the id it used to
    /// carry still resolves — which is what makes the rewrite safe to do one store at a time.
    #[test]
    fn a_rename_repoints_the_definitions_and_leaves_the_old_id_resolving() {
        let config = scratch("all");
        let tools = Tools::with_config(config.clone());
        let uuid = "ec5bd98c-35c1-4e9e-ba25-5c2dbd3d5a99";
        // A tool created under the old rule: a UUID directory with a name beside it.
        std::fs::create_dir_all(tools.dir().join(uuid)).expect("mkdir");
        std::fs::write(
            tools.dir().join(uuid).join("config.toml"),
            "name = \"confirm-sync\"\nruntime = \"sh\"\n",
        )
        .expect("manifest");

        let agents = adi_agents::Agents::with_config(config.clone());
        agents
            .save(
                "adi-agent",
                adi_agents::StoredAgentManifest {
                    backend: "process:claude".into(),
                    bin_tools: vec![uuid.to_string(), "sys-tasks".into()],
                    ..Default::default()
                },
            )
            .expect("agent");
        // An agent that never had the tool must not be touched.
        agents
            .save(
                "other",
                adi_agents::StoredAgentManifest {
                    backend: "process:claude".into(),
                    bin_tools: vec!["sys-tasks".into()],
                    ..Default::default()
                },
            )
            .expect("other agent");

        let report = rename_tool(&config, uuid, "confirm-sync").expect("rename");
        assert!(report.is_clean(), "{:?}", report.warnings);
        assert_eq!(report.tool.id, "confirm-sync");
        assert_eq!(report.from, uuid);
        assert_eq!(report.agents, 1);

        let touched = agents.get("adi-agent").expect("get").expect("present");
        assert_eq!(touched.manifest.bin_tools, ["confirm-sync", "sys-tasks"]);
        let untouched = agents.get("other").expect("get").expect("present");
        assert_eq!(untouched.manifest.bin_tools, ["sys-tasks"]);

        // The old id still names the tool, and still puts the shim on an agent's PATH.
        assert_eq!(
            tools.get(uuid).expect("get").expect("present").id,
            "confirm-sync"
        );
        let bin = tools
            .sync_agent_bin("legacy", &[uuid.to_string()])
            .expect("agent bin");
        assert!(bin.join("confirm-sync").exists());
    }

    /// The registry move is the gate: a refused rename must leave every definition untouched, or a
    /// failed attempt would scatter references across two ids.
    #[test]
    fn a_refused_rename_rewrites_nothing() {
        let config = scratch("refused");
        let tools = Tools::with_config(config.clone());
        let a = tools.create_file("A", None, "sh", None, None).expect("a");
        tools.create_file("B", None, "sh", None, None).expect("b");

        let agents = adi_agents::Agents::with_config(config.clone());
        agents
            .save(
                "solver",
                adi_agents::StoredAgentManifest {
                    backend: "process:claude".into(),
                    bin_tools: vec![a.id.clone()],
                    ..Default::default()
                },
            )
            .expect("agent");

        assert!(matches!(
            rename_tool(&config, &a.id, "b"),
            Err(adi_tools::Error::Exists(_))
        ));
        assert_eq!(
            agents
                .get("solver")
                .expect("get")
                .expect("present")
                .manifest
                .bin_tools,
            [a.id]
        );
    }
}
