//! Renaming a project — the one project operation no single store can finish on its own.
//!
//! A project's id is its directory name under `projects/`, and it is *also* the word half the
//! store writes down about it: a tool or an agent filed under it, a trigger's allowlist, its
//! encrypted secrets, its database, its knowledge bases. [`adi_projects`] owns the registry and
//! nothing else, so a rename that stopped there would leave every one of those pointing at an id
//! nothing answers to — and each of them fails *quietly*: a run starts in the wrong directory, an
//! attached secret injects nothing, a base drops out of an agent's wish list.
//!
//! So the rename lives here, where every store is in reach, and each store owns the part of it
//! only that store can do (re-encrypting a secret under its new scope, moving a database with its
//! sidecars). This is the single place that list of followers exists — the CLI and the control
//! panel both come through here rather than each remembering it.
//!
//! **The registry moves first.** Everything downstream is a reference to the project, so a
//! reference is never re-pointed at an id the move then failed to produce. What follows is
//! reported rather than raised: the project *has* been renamed by then, and a failure to carry one
//! store across is a thing to be told about and fixed, not a reason to claim the rename didn't
//! happen.

use adi_config::Config;
use adi_projects::{Project, Projects, Result};

/// What a [`rename_project`] did, store by store — the receipt the CLI prints and the panel
/// flashes. Counts rather than lists: the point is that nothing was left behind.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectRenamed {
    /// The project under its new id.
    pub project: Project,
    /// The id it had before.
    pub from: String,
    /// Sub-projects whose `parent` now names the new id.
    pub subprojects: usize,
    /// Tools re-filed under the new id.
    pub tools: usize,
    /// Agent definitions re-pointed at it (their project, secrets, and knowledge bases).
    pub agents: usize,
    /// Triggers re-pointed at it (their project and `trigger_on` allowlists).
    pub triggers: usize,
    /// Project-scoped secrets re-encrypted into the new scope.
    pub secrets: usize,
    /// Knowledge bases moved to the new scope.
    pub knowledge: usize,
    /// Whether the project had a database, and it moved.
    pub database: bool,
    /// Stores that could not be carried across, each said in a sentence. Empty on a clean rename;
    /// anything here names a reference still pointing at the old id.
    pub warnings: Vec<String>,
}

impl ProjectRenamed {
    /// Whether every store followed the rename.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }
}

/// Rename a project from `from` to `to` across the whole store: the registry first, then every
/// store that files something under a project id.
///
/// Renaming a project to the id it already has is a no-op. The new id must be a safe single path
/// segment (the rule every store applies) and must not already be taken.
///
/// # Errors
/// [`adi_projects::Error`] from the registry move — an unsafe id, an unregistered `from`, an
/// occupied `to`, or an I/O failure. A store that fails to *follow* the move is reported in
/// [`ProjectRenamed::warnings`], not raised: by then the project has been renamed.
pub fn rename_project(config: &Config, from: &str, to: &str) -> Result<ProjectRenamed> {
    let renamed = Projects::with_config(config.clone()).rename(from, to)?;
    let mut report = ProjectRenamed {
        project: renamed.project,
        from: from.to_string(),
        subprojects: renamed.subprojects,
        tools: 0,
        agents: 0,
        triggers: 0,
        secrets: 0,
        knowledge: 0,
        database: false,
        warnings: Vec::new(),
    };
    if from == to {
        return Ok(report);
    }

    report.tools = follow(
        &mut report.warnings,
        "tools",
        adi_tools::Tools::with_config(config.clone()).rename_project(from, to),
    );
    report.agents = follow(
        &mut report.warnings,
        "agent definitions",
        adi_agents::Agents::with_config(config.clone()).rename_project(from, to),
    );
    report.triggers = follow(
        &mut report.warnings,
        "triggers",
        adi_triggers::Triggers::with_config(config.clone()).rename_project(from, to),
    );
    report.secrets = follow(
        &mut report.warnings,
        "secrets",
        adi_secrets::Secrets::with_config(config.clone()).rename_project(from, to),
    );
    report.knowledge = follow(
        &mut report.warnings,
        "knowledge bases",
        adi_knowledge::KnowledgeStore::with_config(config.clone()).rename_project(from, to),
    );
    report.database = follow(
        &mut report.warnings,
        "the database",
        adi_db::Db::with_config(config.clone()).rename_project(from, to),
    );
    Ok(report)
}

/// Take one store's answer: its result on success, or its default with a sentence naming what was
/// left behind. Generic over the answer because a store reports either a count or a yes/no, and
/// over the error because each store has its own family.
fn follow<T: Default, E: std::fmt::Display>(
    warnings: &mut Vec<String>,
    what: &str,
    result: std::result::Result<T, E>,
) -> T {
    match result {
        Ok(value) => value,
        Err(e) => {
            warnings.push(format!(
                "{what} still point at the old id — they could not be moved: {e}"
            ));
            T::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-core-rename-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    /// The whole point of the operation: after it, nothing in the store still names the old id.
    #[test]
    fn a_rename_carries_every_store_that_files_something_under_the_project() {
        let config = scratch("all");
        let projects = Projects::with_config(config.clone());
        projects
            .create_with_id("old", Some("Old".into()), None, None)
            .expect("project");
        projects
            .create_with_id("kid", None, None, Some("old".into()))
            .expect("sub-project");

        let tools = adi_tools::Tools::with_config(config.clone());
        tools
            .create_file("Deploy", None, "sh", Some("old".into()), None)
            .expect("tool");

        let agents = adi_agents::Agents::with_config(config.clone());
        let mut manifest = adi_agents::StoredAgentManifest {
            backend: "process:claude".into(),
            project: Some("old".into()),
            knowledge: vec!["project:old/runbook".into()],
            ..Default::default()
        };
        manifest.secrets.push(adi_agents::SecretAttachment {
            project: Some("old".into()),
            name: "API_KEY".into(),
        });
        agents.save("solver", manifest).expect("agent");

        let triggers = adi_triggers::Triggers::with_config(config.clone());
        triggers
            .save(
                "nightly",
                adi_triggers::TriggerManifest {
                    kind: adi_triggers::KIND_BACKGROUND.into(),
                    code: "true".into(),
                    project: Some("old".into()),
                    ..Default::default()
                },
            )
            .expect("trigger");

        let secrets = adi_secrets::Secrets::with_config(config.clone());
        secrets
            .set(Some("old"), "API_KEY", "s3cr3t", None)
            .expect("secret");

        let knowledge = adi_knowledge::KnowledgeStore::with_config(config.clone());
        knowledge
            .ensure_base(&"project:old/runbook".parse().expect("base id"))
            .expect("base");

        let db = adi_db::Db::with_config(config.clone());
        db.exec(Some("old"), "create table t (body text)", &[])
            .expect("table");

        let report = rename_project(&config, "old", "new").expect("rename");
        assert!(report.is_clean(), "{:?}", report.warnings);
        assert_eq!(report.project.id, "new");
        assert_eq!(report.from, "old");
        assert_eq!(report.subprojects, 1);
        assert_eq!(report.tools, 1);
        assert_eq!(report.agents, 1);
        assert_eq!(report.triggers, 1);
        assert_eq!(report.secrets, 1);
        assert_eq!(report.knowledge, 1);
        assert!(report.database);

        // The old id is not gone, it is an alias: everything reachable from here was re-pointed at
        // the new one, and the id itself keeps resolving for everything that was not.
        assert_eq!(
            projects.get("old").expect("get").expect("present").id,
            "new"
        );
        assert_eq!(
            projects
                .get("kid")
                .expect("get")
                .expect("present")
                .manifest
                .parent
                .as_deref(),
            Some("new")
        );
        assert_eq!(
            tools.list().expect("tools")[0].manifest.project.as_deref(),
            Some("new")
        );
        let agent = agents.get("solver").expect("get").expect("present");
        assert_eq!(agent.manifest.project.as_deref(), Some("new"));
        assert_eq!(agent.manifest.knowledge, ["project:new/runbook"]);
        assert_eq!(agent.manifest.secrets[0].project.as_deref(), Some("new"));
        assert_eq!(
            triggers
                .get("nightly")
                .expect("get")
                .expect("present")
                .manifest
                .project
                .as_deref(),
            Some("new")
        );
        assert_eq!(
            secrets
                .reveal(Some("new"), "API_KEY")
                .expect("reveal")
                .as_deref(),
            Some("s3cr3t")
        );
        assert!(
            knowledge
                .get_base(&"project:new/runbook".parse().expect("base id"))
                .expect("get")
                .is_some()
        );
        assert!(
            db.tables(Some("new"))
                .expect("tables")
                .iter()
                .any(|t| t.name == "t")
        );
    }

    /// The registry move is the gate. A refused rename must leave every follower untouched, or a
    /// failed attempt would scatter references across two ids.
    #[test]
    fn a_refused_rename_moves_nothing() {
        let config = scratch("refused");
        let projects = Projects::with_config(config.clone());
        projects
            .create_with_id("old", None, None, None)
            .expect("old");
        projects
            .create_with_id("taken", None, None, None)
            .expect("taken");
        let tools = adi_tools::Tools::with_config(config.clone());
        let tool = tools
            .create_file("Deploy", None, "sh", Some("old".into()), None)
            .expect("tool");

        assert!(matches!(
            rename_project(&config, "old", "taken"),
            Err(adi_projects::Error::Exists(_))
        ));
        assert!(projects.get("old").expect("get").is_some());
        assert_eq!(
            tools
                .get(&tool.id)
                .expect("get")
                .expect("present")
                .manifest
                .project
                .as_deref(),
            Some("old")
        );
    }
}
