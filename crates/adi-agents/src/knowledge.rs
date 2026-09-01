//! What a run is told about its knowledge: the memory it keeps, and the bases it may read.
//!
//! Two fields on an agent decide this — `memory` and `knowledge` — and until now they were
//! storage and nothing else. This is the layer that makes them mean something at launch:
//!
//! * `memory = true` **creates** `agent:<name>/memory` if it isn't there. A toggle that only
//!   recorded an intention would leave the agent writing to a base that does not exist, and it
//!   would find that out mid-turn.
//! * `knowledge = [...]` is resolved through the three isolation levels, so what reaches the run
//!   is what it can actually read — another project's base drops out, another agent's memory
//!   comes through read-only. The list is a **wish list**, and this is where the wishing ends.
//!
//! The run reaches all of it through the **CLI**, exactly like the database, the secrets, and the
//! task tree: `adi-knowledge` on its `PATH`, with `$ADI_MEMORY` and `$ADI_KNOWLEDGE` naming the
//! bases. There is deliberately no model-facing tool — a capability an agent reaches by running a
//! command is one surface to learn, one place to fix, and it works the same in a script the agent
//! writes as in a command it types.
//!
//! Everything here is **best-effort**. A store that cannot be opened, a base that cannot be
//! created — none of it blocks a launch. The agent then simply is not told about knowledge it
//! hasn't got, which is the honest failure: the alternative is refusing to start a run over a
//! feature it may never have used.

use std::fmt::Write as _;

use adi_config::Config;
use adi_knowledge::{BaseId, KnowledgeStore, Scope, resolve_agent_bases};

use crate::agent::StoredAgent;

/// The env var naming the agent's own memory base, when it has one.
const MEMORY_ENV: &str = "ADI_MEMORY";

/// The env var listing every base the run may read, space-separated, memory first.
const KNOWLEDGE_ENV: &str = "ADI_KNOWLEDGE";

/// What one run gets: its memory base (if any) and every base it may read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RunKnowledge {
    /// `agent:<name>/memory`, present only when the agent has memory **and** the base exists —
    /// so the variable never names something the agent would fail to write to.
    pub(crate) memory: Option<String>,
    /// Every base this run may read, memory first. Already filtered by the isolation levels.
    pub(crate) bases: Vec<String>,
}

impl RunKnowledge {
    /// Whether this run has any knowledge at all — the gate on saying anything about it.
    pub(crate) fn is_empty(&self) -> bool {
        self.memory.is_none() && self.bases.is_empty()
    }

    /// The variables the run carries. A script the agent writes reads these; prose it might have
    /// been given, it might not.
    pub(crate) fn env(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        if let Some(memory) = &self.memory {
            vars.push((MEMORY_ENV.to_string(), memory.clone()));
        }
        if !self.bases.is_empty() {
            vars.push((KNOWLEDGE_ENV.to_string(), self.bases.join(" ")));
        }
        vars
    }
}

/// Resolve an agent's knowledge for a launch, creating its memory base if it is owed one.
///
/// The one write in the whole module, and it is deliberate: this is called from the launch path,
/// which is already the write path (see `Agents::spec_in`).
pub(crate) fn resolve(config: &Config, agent: &StoredAgent) -> RunKnowledge {
    let manifest = &agent.manifest;
    if !manifest.memory && manifest.knowledge.is_empty() {
        return RunKnowledge::default();
    }

    let store = KnowledgeStore::with_config(config.clone());
    // Make the memory real before naming it. If that fails the agent is told nothing about a
    // memory, rather than handed a variable pointing at a base that is not there.
    let memory = manifest.memory.then(|| store.ensure_memory(&agent.name));
    let memory = match memory {
        Some(Ok(base)) => Some(base.id.to_string()),
        Some(Err(e)) => {
            tracing::warn!(agent = %agent.name, error = %e, "could not open this agent's memory");
            None
        }
        None => None,
    };

    let project = manifest
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let bases = match resolve_agent_bases(
        &agent.name,
        project,
        &manifest.knowledge,
        memory.is_some(),
    ) {
        Ok(ids) => ids.iter().map(ToString::to_string).collect(),
        Err(e) => {
            tracing::warn!(agent = %agent.name, error = %e, "could not resolve knowledge bases");
            Vec::new()
        }
    };

    RunKnowledge { memory, bases }
}

/// One `knowledge` entry, re-addressed after its project was renamed — `Some` only when the entry
/// names a base in project `from`. Anything else (a global base, another project's, an agent's, or
/// a line that does not parse at all) comes back `None` and is left exactly as written.
///
/// Rewriting the entry rather than the whole list is what keeps a wish list a wish list: an agent
/// that named a base it could never read keeps naming it. A bare `project:<from>` normalizes to
/// `project:<to>/<default base>` on the way through — the same base, spelled in full.
pub(crate) fn rescope_base(entry: &str, from: &str, to: &str) -> Option<String> {
    let id: BaseId = entry.parse().ok()?;
    if !matches!(&id.scope, Scope::Project { project } if project == from) {
        return None;
    }
    Some(
        BaseId::new(Scope::project(to).ok()?, id.name)
            .ok()?
            .to_string(),
    )
}

/// The prompt section telling the run what it knows and how to reach it.
///
/// Stated in prose *and* exported as variables, for the same reason the working directory is: the
/// agent reads prose, the script it writes reads the environment, and neither can be told the
/// other's way.
///
/// Empty when the agent has no knowledge — a run that has none should not be given a paragraph
/// about a feature it does not have.
pub(crate) fn block(knowledge: &RunKnowledge) -> String {
    if knowledge.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# What you know\n\n\
         You have a knowledge base: text notes, searched by meaning rather than by keyword, so \
         ask it the question you actually have — `adi-knowledge search \"why does the front door \
         502\"` — not the words you expect to find. It knows who you are from your environment, \
         so you never pass your own name.",
    );

    if let Some(memory) = &knowledge.memory {
        let _ = write!(
            out,
            "\n\n**You have a memory of your own**, `{memory}` (`${MEMORY_ENV}`). You alone write \
             it; every other agent may read it. Write to it when you learn something that was \
             not obvious and will be true next time — how this deployment actually behaves, why \
             an approach that looked right is wrong, a command that turned out to be the one that \
             works:\n\n\
             ```\n\
             adi-knowledge add \"$ADI_MEMORY\" -t \"The front door 502s on a stale route\" \\\n  \
             -b \"Rebuild adi-hive after editing hive.yaml; a reload is not enough.\"\n\
             ```\n\n\
             Not a log of what you did — the run's own transcript already holds that. A note is \
             worth writing when it would save the *next* run the work you just did."
        );
    }

    let readable: Vec<&String> = knowledge
        .bases
        .iter()
        .filter(|id| Some(*id) != knowledge.memory.as_ref())
        .collect();
    if !readable.is_empty() {
        let list = readable
            .iter()
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            out,
            "\n\nYou can also read: {list} (`${KNOWLEDGE_ENV}`). A base named `agent:<someone>/…` \
             is another agent's memory — readable, never yours to change."
        );
    }

    // Written after a run went looking for a memo it had been handed as `business/memos/…` — a
    // path valid in its *caller's* working directory and meaningless in its own. It fell back to a
    // `find` over the home directory, missed by one level of `-maxdepth`, and concluded the file
    // did not exist. The base is the one channel that survives the crossing, so say so here, where
    // an agent is already being told what it knows.
    let _ = write!(
        out,
        "\n\n**A note reaches another agent; a file path does not.** A run you hand work to \
         starts in its own working directory, so a path you wrote relative to yours resolves to \
         nothing there — and it will go hunting for a file it will never find. Cite the base and \
         the note's title and let it search. If it truly needs the file, give the absolute path, \
         or put the passage it needs in the brief itself."
    );

    let _ = write!(
        out,
        "\n\nSearch before you set out on anything non-trivial here: the answer may already have \
         been worked out by a run that is over. `adi-knowledge --help` has the rest."
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::StoredAgentManifest;

    fn agent(name: &str, memory: bool, knowledge: &[&str], project: Option<&str>) -> StoredAgent {
        StoredAgent {
            name: name.to_string(),
            manifest: StoredAgentManifest {
                backend: "harness:adi".into(),
                memory,
                knowledge: knowledge.iter().map(ToString::to_string).collect(),
                project: project.map(ToString::to_string),
                ..Default::default()
            },
        }
    }

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-knowledge-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    #[test]
    fn rescope_base_moves_only_the_renamed_projects_entries() {
        assert_eq!(
            rescope_base("project:old/runbook", "old", "new").as_deref(),
            Some("project:new/runbook")
        );
        // A bare project scope means that project's default base, and says so once it moves.
        assert_eq!(
            rescope_base("project:old", "old", "new").as_deref(),
            Some("project:new/default")
        );
        for untouched in [
            "global/runbooks",
            "agent:reviewer/memory",
            "project:other/runbook",
            "not a base id at all",
            "",
        ] {
            assert_eq!(rescope_base(untouched, "old", "new"), None, "{untouched}");
        }
    }

    /// An agent with neither setting must not touch the knowledge store at all — no directory,
    /// no variables, no paragraph.
    #[test]
    fn an_agent_without_knowledge_is_told_nothing_and_nothing_is_created() {
        let config = scratch("none");
        let k = resolve(&config, &agent("solver", false, &[], None));
        assert!(k.is_empty());
        assert!(k.env().is_empty());
        assert!(block(&k).is_empty());
        assert!(
            !config.module("knowledge").dir().exists(),
            "a store was created for an agent that asked for none"
        );
    }

    /// The toggle's whole job: the base exists after a launch, not merely as an intention.
    #[test]
    fn memory_is_created_at_launch_and_named_in_the_environment() {
        let config = scratch("memory");
        let k = resolve(&config, &agent("solver", true, &[], None));
        assert_eq!(k.memory.as_deref(), Some("agent:solver/memory"));

        let store = KnowledgeStore::with_config(config.clone());
        let id: adi_knowledge::BaseId = "agent:solver/memory".parse().expect("id");
        assert!(store.get_base(&id).expect("get").is_some());

        let env = k.env();
        assert!(env.contains(&(MEMORY_ENV.into(), "agent:solver/memory".into())));
        assert!(
            env.iter()
                .any(|(key, v)| key == KNOWLEDGE_ENV && v.contains("agent:solver/memory"))
        );
    }

    /// The wish list ends here: what reaches the run is what the isolation levels allow.
    #[test]
    fn configured_bases_are_filtered_by_what_the_agent_can_actually_read() {
        let config = scratch("filter");
        let store = KnowledgeStore::with_config(config.clone());
        for id in [
            "global/runbooks",
            "project:acme/notes",
            "project:other/notes",
            "agent:reviewer/memory",
        ] {
            store
                .ensure_base(&id.parse().expect("id"))
                .expect("ensure base");
        }

        let k = resolve(
            &config,
            &agent(
                "solver",
                false,
                &[
                    "global/runbooks",
                    "project:acme/notes",
                    "project:other/notes",   // another project's — dropped
                    "agent:reviewer/memory", // another agent's — kept, read-only
                    "nonsense::",            // unparseable — dropped
                ],
                Some("acme"),
            ),
        );
        assert_eq!(
            k.bases,
            vec![
                "global/runbooks",
                "project:acme/notes",
                "agent:reviewer/memory"
            ]
        );
        assert_eq!(k.memory, None, "memory was off");
    }

    /// A memory that could not be created must not be named — the variable would point at
    /// nothing and the agent would find out mid-turn.
    #[test]
    fn a_memory_that_could_not_be_made_is_not_promised() {
        let config = scratch("broken");
        // A file where the knowledge module's directory belongs: every base write under it fails.
        let dir = config.module("knowledge").dir().to_path_buf();
        std::fs::create_dir_all(dir.parent().expect("parent")).expect("mkdir");
        std::fs::write(&dir, b"not a directory").expect("write");

        let k = resolve(&config, &agent("solver", true, &[], None));
        assert_eq!(k.memory, None);
        assert!(k.env().iter().all(|(key, _)| key != MEMORY_ENV));
    }

    #[test]
    fn the_block_names_the_memory_and_the_readable_bases() {
        let k = RunKnowledge {
            memory: Some("agent:solver/memory".into()),
            bases: vec!["agent:solver/memory".into(), "global/runbooks".into()],
        };
        let text = block(&k);
        assert!(text.contains("agent:solver/memory"));
        assert!(text.contains("$ADI_MEMORY"));
        assert!(text.contains("global/runbooks"));
        assert!(text.contains("adi-knowledge search"));
        assert_eq!(
            text.matches("global/runbooks").count(),
            1,
            "a base should be named once"
        );
    }

    #[test]
    fn a_read_only_agent_gets_no_memory_paragraph() {
        let k = RunKnowledge {
            memory: None,
            bases: vec!["global/runbooks".into()],
        };
        let text = block(&k);
        assert!(!text.contains("memory of your own"));
        assert!(text.contains("global/runbooks"));
    }

    /// The one thing a run cannot work out for itself: a path it hands to another agent is read
    /// from a different working directory. It belongs to every agent that has a base to cite,
    /// whether or not that agent also writes one.
    #[test]
    fn every_agent_with_a_base_is_told_how_to_cite_it_to_another_run() {
        for k in [
            RunKnowledge {
                memory: Some("agent:solver/memory".into()),
                bases: vec!["agent:solver/memory".into()],
            },
            RunKnowledge {
                memory: None,
                bases: vec!["global/runbooks".into()],
            },
        ] {
            let text = block(&k);
            assert!(text.contains("A note reaches another agent"), "{text}");
            assert!(text.contains("absolute path"), "{text}");
        }
    }
}
