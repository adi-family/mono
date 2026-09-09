//! Moving agent definitions from one shape to the next, once, in order.
//!
//! Every definition carries the shape it is written in (`version`, see
//! [`MANIFEST_VERSION`](crate::agent::MANIFEST_VERSION)). This module is the list of steps between
//! those numbers and the runner that applies the ones an agent has not had yet — so an upgrade is
//! something the store *records having done*, rather than something an operator remembers running.
//!
//! Three properties are the point of the version being on the file at all:
//!
//! * **A step runs once.** An agent stamped 2 is not offered the 1→2 step again, so a migration
//!   that is not idempotent by construction is still safe to re-run.
//! * **A newer store is refused, not guessed at.** A definition stamped above what this binary
//!   knows was written by a later version of ADI; this one reports it and leaves it alone rather
//!   than writing a shape it does not understand back over it.
//! * **An edit is not an upgrade.** [`Agents::save`](crate::Agents::save) preserves the stored
//!   version, so saving a legacy agent from the panel cannot mark it migrated. Only
//!   `save_migrated`, which lives behind this module, moves the number.
//!
//! The steps are deliberately *whole-store* operations that stamp per agent: 1→2 has to create the
//! shared backends before it can point any agent at one, and a per-agent runner would have had to
//! invent them 80 times.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent::MANIFEST_VERSION;
use crate::error::Result;
use crate::llm::{LlmBackends, migrate as llm_migrate};

/// One move between two adjacent shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// The shape this step reads.
    pub from: u32,
    /// The shape it leaves behind — always `from + 1`, so a store is never two shapes at once.
    pub to: u32,
    /// The short name printed in a plan.
    pub name: &'static str,
    /// What it actually does to a definition, in a sentence, for somebody deciding whether to run
    /// it.
    pub what: &'static str,
}

/// Every step this binary knows, in the order they must run.
pub const STEPS: [Step; 1] = [Step {
    from: 1,
    to: 2,
    name: "llm-backends",
    what: "lifts the model, the login and the dials out of the agent into a named LLM backend, and \
           puts that backend at the head of the agent's list",
}];

/// What one agent needs, if anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// The agent's name.
    pub agent: String,
    /// The shape it is in now.
    pub from: u32,
    /// The steps it has not had, in order, by [`Step::name`].
    pub steps: Vec<String>,
}

/// The whole store, read and sorted into what needs doing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Agents below [`MANIFEST_VERSION`](crate::agent::MANIFEST_VERSION), worst first.
    pub pending: Vec<Pending>,
    /// How many are already current — reported rather than listed, because on a migrated store
    /// that is every agent and the list would be the noise the plan is trying to cut through.
    pub current: usize,
    /// Agents stamped **above** what this binary knows, and the version each claims. Nothing here
    /// is touched; it is reported so an operator running an old binary against a new store finds
    /// out from the plan rather than from the damage.
    pub ahead: BTreeMap<String, u32>,
}

impl Plan {
    /// Whether applying this plan would write anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The distinct steps this plan will run, in order — what a summary line names.
    #[must_use]
    pub fn steps(&self) -> Vec<&'static Step> {
        STEPS
            .iter()
            .filter(|step| {
                self.pending
                    .iter()
                    .any(|agent| agent.steps.iter().any(|name| name == step.name))
            })
            .collect()
    }
}

/// What applying it did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// The steps that ran, by name.
    pub steps: Vec<String>,
    /// How many definitions were rewritten and re-stamped.
    pub agents: usize,
    /// What each step had to say for itself — the backends 1→2 created, and so on.
    pub notes: Vec<String>,
}

/// Read every definition and work out what it still needs.
///
/// # Errors
/// [`Error::Io`](crate::Error::Io) or [`Error::Parse`](crate::Error::Parse) when the store cannot
/// be listed.
pub fn plan(agents: &crate::Agents) -> Result<Plan> {
    let mut plan = Plan::default();
    for agent in agents.list()? {
        let shape = agent.manifest.shape();
        if shape > MANIFEST_VERSION {
            plan.ahead.insert(agent.name, shape);
            continue;
        }
        if shape == MANIFEST_VERSION {
            plan.current += 1;
            continue;
        }
        plan.pending.push(Pending {
            agent: agent.name,
            from: shape,
            steps: STEPS
                .iter()
                .filter(|step| step.from >= shape)
                .map(|step| step.name.to_string())
                .collect(),
        });
    }
    plan.pending.sort_by(|a, b| a.from.cmp(&b.from).then(a.agent.cmp(&b.agent)));
    Ok(plan)
}

/// Run every pending step, then stamp each agent it covered.
///
/// The stamp lands **after** the step's own write, per agent, so an interruption leaves a store
/// that is half migrated and *says so* — the next run picks up exactly the agents that were not
/// reached. Re-running on a finished store writes nothing.
///
/// # Errors
/// Whatever the step itself returns, or [`Error::Io`](crate::Error::Io) writing a definition back.
pub fn apply(agents: &crate::Agents, registry: &LlmBackends) -> Result<Applied> {
    let plan = plan(agents)?;
    let mut applied = Applied::default();
    for step in plan.steps() {
        let covered: Vec<&Pending> = plan
            .pending
            .iter()
            .filter(|agent| agent.steps.iter().any(|name| name == step.name))
            .collect();
        match step.to {
            2 => run_llm_backends(agents, registry, &mut applied)?,
            // Unreachable while STEPS holds one entry; a step added without a body should say so
            // loudly rather than silently stamp agents it never touched.
            other => {
                return Err(crate::Error::Arguments(format!(
                    "migration step {} ({}) has no implementation",
                    other, step.name
                )));
            }
        }
        applied.steps.push(step.name.to_string());
        for agent in covered {
            stamp(agents, &agent.agent, step.to)?;
        }
    }
    // Counted once per definition, not once per step: an agent two shapes behind is carried by two
    // steps and is still one agent.
    applied.agents = plan.pending.len();
    Ok(applied)
}

/// Step 1 → 2. Delegates to [`llm_migrate`], which already knows how to read an agent's own model
/// configuration and is idempotent on an agent that has a chain — so an operator who ran
/// `llm migrate` by hand before this existed gets a stamp and no rewrite, which is the truth.
fn run_llm_backends(
    agents: &crate::Agents,
    registry: &LlmBackends,
    applied: &mut Applied,
) -> Result<()> {
    let plan = llm_migrate::plan(agents, registry)?;
    let created: Vec<&str> = plan
        .moves
        .iter()
        .filter(|mv| mv.created)
        .map(|mv| mv.backend.as_str())
        .collect();
    let changed = llm_migrate::apply(agents, registry, &plan)?;
    applied.notes.push(if plan.moves.is_empty() {
        "llm-backends: every agent already lists a backend; nothing to lift".to_string()
    } else {
        format!(
            "llm-backends: {changed} agent(s) rewritten, {} backend(s) created{}",
            created.len(),
            if created.is_empty() {
                String::new()
            } else {
                format!(" ({})", created.join(", "))
            }
        )
    });
    Ok(())
}

/// Re-read a definition and write it back at `version`, changing nothing else.
fn stamp(agents: &crate::Agents, name: &str, version: u32) -> Result<()> {
    let Some(agent) = agents.get(name)? else {
        // Deleted between plan and apply. Not an error: the store is in the state the plan wanted.
        return Ok(());
    };
    agents.save_migrated(name, agent.manifest, version)
}

/// The oldest shape anything in this store is in — what an operator means by "which version is my
/// store on". [`MANIFEST_VERSION`] when it is empty, because a store with no agents is not behind.
///
/// # Errors
/// As [`plan`].
pub fn store_version(agents: &crate::Agents) -> Result<u32> {
    Ok(agents
        .list()?
        .iter()
        .map(|agent| agent.manifest.shape())
        .min()
        .unwrap_or(MANIFEST_VERSION))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentManifest, LEGACY_VERSION, RawAgentArguments};
    use crate::backend::Backend;
    use crate::llm::AgentBackendEntry;
    use adi_config::Config;

    fn scratch(tag: &str) -> crate::Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-migrations-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        crate::Agents::with_config(Config::with_root(root))
    }

    fn legacy(model: &str) -> AgentManifest<RawAgentArguments> {
        let mut arguments = RawAgentArguments::new();
        arguments.insert("model".to_string(), serde_json::json!(model));
        AgentManifest {
            backend: Backend::HarnessClaudeSdk,
            arguments,
            ..Default::default()
        }
    }

    #[test]
    fn a_file_with_no_version_line_reads_as_the_oldest_shape() {
        let manifest = legacy("opus");
        assert_eq!(manifest.version, 0, "nothing writes a zero");
        assert_eq!(manifest.shape(), LEGACY_VERSION);
    }

    #[test]
    fn a_new_agent_is_stamped_current_and_needs_no_migration() {
        let agents = scratch("a_new_agent_is_stamped_current_and_needs_no_migration");
        let mut manifest = legacy("opus");
        manifest.backends = vec![AgentBackendEntry::new("anthropic")];
        agents.save("fresh", manifest).expect("save");

        let saved = agents.get("fresh").expect("get").expect("present");
        assert_eq!(saved.manifest.version, MANIFEST_VERSION);
        assert!(plan(&agents).expect("plan").is_empty());
    }

    #[test]
    fn saving_a_legacy_agent_does_not_pretend_it_was_migrated() {
        let agents = scratch("saving_a_legacy_agent_does_not_pretend_it_was_migrated");
        // Write it the way a pre-version binary did: no stamp, model on the agent.
        agents
            .save_migrated("old", legacy("opus").to_stored().expect("stored"), 0)
            .expect("seed");

        // An ordinary edit — the panel ticking a star — must leave the shape alone.
        let mut manifest = agents.get("old").expect("get").expect("present").manifest;
        manifest.starred = true;
        agents.save("old", manifest).expect("save");

        let saved = agents.get("old").expect("get").expect("present");
        assert!(saved.manifest.starred);
        assert_eq!(saved.manifest.shape(), LEGACY_VERSION, "still legacy");
        assert_eq!(plan(&agents).expect("plan").pending.len(), 1);
    }

    #[test]
    fn a_plan_names_the_steps_an_agent_has_not_had() {
        let agents = scratch("a_plan_names_the_steps_an_agent_has_not_had");
        agents
            .save_migrated("old", legacy("opus").to_stored().expect("stored"), 0)
            .expect("seed");

        let plan = plan(&agents).expect("plan");
        assert_eq!(plan.pending.len(), 1);
        assert_eq!(plan.pending[0].agent, "old");
        assert_eq!(plan.pending[0].from, LEGACY_VERSION);
        assert_eq!(plan.pending[0].steps, vec!["llm-backends".to_string()]);
        assert_eq!(plan.current, 0);
        assert!(plan.ahead.is_empty());
    }

    #[test]
    fn migrating_moves_the_model_out_and_stamps_the_file() {
        let agents = scratch("migrating_moves_the_model_out_and_stamps_the_file");
        let registry = LlmBackends::with_config(agents.config().clone());
        agents
            .save_migrated("old", legacy("opus").to_stored().expect("stored"), 0)
            .expect("seed");

        let applied = apply(&agents, &registry).expect("apply");
        assert_eq!(applied.agents, 1);
        assert_eq!(applied.steps, vec!["llm-backends"]);

        let saved = agents.get("old").expect("get").expect("present").manifest;
        assert_eq!(saved.version, MANIFEST_VERSION);
        assert_eq!(saved.backends.len(), 1, "it now names a backend");
        assert!(!saved.arguments.contains_key("model"), "and not a model");
    }

    #[test]
    fn a_second_run_writes_nothing() {
        let agents = scratch("a_second_run_writes_nothing");
        let registry = LlmBackends::with_config(agents.config().clone());
        agents
            .save_migrated("old", legacy("opus").to_stored().expect("stored"), 0)
            .expect("seed");
        apply(&agents, &registry).expect("first");

        let plan = plan(&agents).expect("plan");
        assert!(plan.is_empty());
        assert_eq!(plan.current, 1);
        let applied = apply(&agents, &registry).expect("second");
        assert_eq!(applied.agents, 0);
        assert!(applied.steps.is_empty());
    }

    #[test]
    fn an_agent_that_was_migrated_by_hand_is_stamped_without_being_rewritten() {
        let agents = scratch("an_agent_that_was_migrated_by_hand_is_stamped_without_being_rewritten");
        let registry = LlmBackends::with_config(agents.config().clone());
        // The state this store was actually in: `llm migrate` was run before versions existed, so
        // the agent has its chain but no stamp.
        let mut manifest = legacy("opus").to_stored().expect("stored");
        manifest.arguments.remove("model");
        manifest.backends = vec![AgentBackendEntry::new("anthropic")];
        agents.save_migrated("done", manifest, 0).expect("seed");

        let applied = apply(&agents, &registry).expect("apply");
        assert_eq!(applied.agents, 1, "stamped");
        let saved = agents.get("done").expect("get").expect("present").manifest;
        assert_eq!(saved.version, MANIFEST_VERSION);
        assert_eq!(saved.backends, vec![AgentBackendEntry::new("anthropic")]);
        assert!(
            registry.list().expect("list").is_empty(),
            "and no backend was invented for an agent that already had one"
        );
    }

    #[test]
    fn a_definition_from_a_newer_binary_is_reported_and_left_alone() {
        let agents = scratch("a_definition_from_a_newer_binary_is_reported_and_left_alone");
        let registry = LlmBackends::with_config(agents.config().clone());
        let mut manifest = legacy("opus").to_stored().expect("stored");
        manifest.backends = vec![AgentBackendEntry::new("anthropic")];
        agents
            .save_migrated("from-the-future", manifest, MANIFEST_VERSION + 7)
            .expect("seed");

        let plan = plan(&agents).expect("plan");
        assert!(plan.pending.is_empty(), "nothing to do to it");
        assert_eq!(plan.ahead.get("from-the-future"), Some(&(MANIFEST_VERSION + 7)));

        apply(&agents, &registry).expect("apply");
        let saved = agents.get("from-the-future").expect("get").expect("present");
        assert_eq!(
            saved.manifest.version,
            MANIFEST_VERSION + 7,
            "its version was not walked backwards"
        );
    }

    #[test]
    fn the_store_version_is_the_oldest_agent_in_it() {
        let agents = scratch("the_store_version_is_the_oldest_agent_in_it");
        assert_eq!(
            store_version(&agents).expect("empty"),
            MANIFEST_VERSION,
            "an empty store is not behind"
        );

        let mut current = legacy("opus").to_stored().expect("stored");
        current.backends = vec![AgentBackendEntry::new("anthropic")];
        agents
            .save_migrated("new", current, MANIFEST_VERSION)
            .expect("seed");
        agents
            .save_migrated("old", legacy("opus").to_stored().expect("stored"), 0)
            .expect("seed");

        assert_eq!(store_version(&agents).expect("mixed"), LEGACY_VERSION);
    }

    #[test]
    fn every_step_moves_exactly_one_version_and_they_join_up() {
        let mut expected = LEGACY_VERSION;
        for step in &STEPS {
            assert_eq!(step.from, expected, "steps must be contiguous");
            assert_eq!(step.to, step.from + 1, "one shape at a time");
            expected = step.to;
        }
        assert_eq!(
            expected, MANIFEST_VERSION,
            "the last step must land on the version this binary writes"
        );
    }
}
