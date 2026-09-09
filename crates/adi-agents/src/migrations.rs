//! Moving agent definitions from one shape to the next, once, in order.
//!
//! Every definition carries the shape it is written in (`version`, see
//! [`MANIFEST_VERSION`](crate::agent::MANIFEST_VERSION)). This module is the list of steps between
//! those numbers and the runner that applies the ones an agent has not had yet — so an upgrade is
//! something the store *records having done*, rather than something an operator remembers running.
//!
//! **The chain starts at nothing.** A file with no `version` line is [`UNVERSIONED`], not "version
//! 1": the shape is inferred rather than stated, and `0 → 1` is the step that states it. Every
//! later shape change appends one more step — `1 → 2`, `2 → 3` — and a store walks the whole chain
//! from wherever it is, one version at a time, whether it was last touched yesterday or two
//! releases ago.
//!
//! It runs **on boot** ([`on_boot`], called by `adi-app` before anything reads an agent) as well as
//! from `adi-mono agents migrate`, so a definition is brought forward by the thing that opens the
//! store rather than by somebody remembering to.
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

use crate::agent::{MANIFEST_VERSION, UNVERSIONED};
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

/// Every step this binary knows, in the order they must run — from nothing, upward, one version at
/// a time. Adding a shape change means appending a step here and bumping
/// [`MANIFEST_VERSION`](crate::agent::MANIFEST_VERSION); the two are checked against each other by
/// a test, so a step that does not join the chain does not compile past `cargo test`.
pub const STEPS: [Step; 3] = [
    Step {
        from: UNVERSIONED,
        to: 1,
        name: "stamp",
        what: "records the shape a definition written before versions existed is already in — it \
               rewrites nothing, it only says so on the file",
    },
    Step {
        from: 1,
        to: 2,
        name: "llm-backends",
        what: "lifts the model, the login and the dials out of the agent into a named LLM \
               backend, and puts that backend at the head of the agent's list",
    },
    Step {
        from: 2,
        to: 3,
        name: "runtime",
        what: "takes the runtime off every agent with a chain — it is the one on the backend at \
               the head of it, so a run that fails over to another runner changes runtime with \
               it; an agent listing no backends has nothing to derive from and keeps its own",
    },
];

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
    /// Agents a step could not do, and why: they keep the version they had, so the next run offers
    /// them the same step again once the reason is dealt with. A held-back agent is not a failure
    /// of the migration — it is the migration refusing to guess.
    pub held: BTreeMap<String, String>,
    /// [`Plan::ahead`] carried through, because the caller that never asks for a plan is the one
    /// that most needs to hear this: an old binary booting on a store a newer one has already
    /// migrated writes nothing, and without this it would also *say* nothing, then go on to serve
    /// definitions whose shape it does not know.
    pub ahead: BTreeMap<String, u32>,
}

/// Read every definition and work out what it still needs.
///
/// # Errors
/// [`Error::Io`](crate::Error::Io) or [`Error::Parse`](crate::Error::Parse) when the store cannot
/// be listed.
pub fn plan(agents: &crate::Agents) -> Result<Plan> {
    let mut plan = Plan::default();
    for agent in agents.list()? {
        let shape = agent.manifest.version;
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
    let mut applied = Applied {
        ahead: plan.ahead.clone(),
        ..Applied::default()
    };
    for step in plan.steps() {
        let covered: Vec<&Pending> = plan
            .pending
            .iter()
            .filter(|agent| agent.steps.iter().any(|name| name == step.name))
            .collect();
        match step.to {
            // 0 → 1 has no body on purpose: the shape is already what version 1 describes, and the
            // step exists to write that down. The stamp below is the whole of it.
            1 => {}
            2 => run_llm_backends(agents, registry, &mut applied)?,
            3 => run_runtime(agents, registry, &covered, &mut applied)?,
            // A step added to STEPS without a body should say so loudly rather than silently stamp
            // agents it never touched.
            other => {
                return Err(crate::Error::Arguments(format!(
                    "migration step {} ({}) has no implementation",
                    other, step.name
                )));
            }
        }
        applied.steps.push(step.name.to_string());
        for agent in covered {
            // An agent this step held back keeps the version it had, so it is offered the same step
            // again next time instead of being recorded as having had one it did not.
            if applied.held.contains_key(&agent.agent) {
                continue;
            }
            stamp(agents, &agent.agent, step.to)?;
        }
    }
    // Counted once per definition, not once per step: an agent two shapes behind is carried by two
    // steps and is still one agent.
    applied.agents = plan
        .pending
        .iter()
        .filter(|agent| !applied.held.contains_key(&agent.agent))
        .count();
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

/// Step 2 → 3. Takes the runtime off the agent, having first checked that the chain says the same
/// thing the agent did.
///
/// The check is the whole of the risk. `backend = "harness:adi"` is what an agent has been running
/// on for as long as it has existed, and the replacement — the runtime of the backend at the head
/// of its chain — is only safe to switch to if it *is* the same runtime. Where it is not, the
/// definition is held back with the disagreement named, because silently moving an agent onto
/// another runner is the one outcome nobody could debug afterwards.
fn run_runtime(
    agents: &crate::Agents,
    registry: &LlmBackends,
    covered: &[&Pending],
    applied: &mut Applied,
) -> Result<()> {
    let catalog = crate::llm::catalog(registry.list()?);
    let mut moved = 0usize;
    let mut kept = 0usize;
    for pending in covered {
        let Some(agent) = agents.get(&pending.agent)? else {
            continue;
        };
        // An agent that lists no backends has no chain to take a runtime from, and nothing to
        // disagree with either: at v3 its `backend =` line is its own declaration, which is what
        // keeps a `pty:claude` or `process:codex` agent runnable. So this one is a stamp — the file
        // is already in the shape v3 describes.
        if agent.manifest.backends.is_empty() {
            stamp(agents, &pending.agent, 3)?;
            kept += 1;
            continue;
        }
        // What the file says, not what `get` derived: an unmigrated definition still carries its
        // own, and that is the value this step has to honour.
        let stored = agents
            .raw_manifest(&pending.agent)?
            .and_then(|manifest| manifest.backend);
        let head = agent
            .manifest
            .backends
            .first()
            .and_then(|row| catalog.get(&row.backend))
            .map(|backend| backend.runtime.clone());
        match (stored, head) {
            // It lists backends, and the one it starts on is gone. The chain cannot say what this
            // agent runs on and the file is about to stop saying it either, so hold it: whoever
            // deleted that backend has a decision to make.
            (_, None) => {
                applied.held.insert(
                    pending.agent.clone(),
                    "the backend at the head of its chain does not exist, so there is no runtime \
                     to take from it"
                        .to_string(),
                );
            }
            (Some(was), Some(now)) if was != now && !was.is_unset() => {
                applied.held.insert(
                    pending.agent.clone(),
                    format!(
                        "runs on {was} but its first backend is {now} — put a backend on {was} at \
                         the head of its chain, or change the agent over deliberately"
                    ),
                );
            }
            // Either they agree, or the file never had one to disagree with.
            (_, Some(_)) => {
                let mut manifest = agent.manifest;
                manifest.backend = None;
                agents.save_migrated(&pending.agent, manifest, 3)?;
                moved += 1;
            }
        }
    }
    applied.notes.push(format!(
        "runtime: {moved} agent(s) now take their runtime from the head of their chain, {kept} \
         with no chain keep their own{}",
        if applied.held.is_empty() {
            String::new()
        } else {
            format!(", {} held back", applied.held.len())
        }
    ));
    Ok(())
}

/// Re-read a definition and write it back at `version`, changing nothing else.
fn stamp(agents: &crate::Agents, name: &str, version: u32) -> Result<()> {
    // The file as written, not as read: a stamp changes one number and must put everything else
    // back exactly as it found it — including the runtime a v2 definition still carries.
    let Some(manifest) = agents.raw_manifest(name)? else {
        // Deleted between plan and apply. Not an error: the store is in the state the plan wanted.
        return Ok(());
    };
    agents.save_migrated(name, manifest, version)
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
        .map(|agent| agent.manifest.version)
        .min()
        .unwrap_or(MANIFEST_VERSION))
}

/// Bring the store forward on the way up, before anything reads an agent.
///
/// The migration an operator has to remember is a migration that does not happen: the store is read
/// by the panel, the launcher and every trigger within a second of the app starting, and each of
/// them assumes the current shape. So `adi-app` calls this as it opens the store, and a definition
/// left behind by an older binary is brought forward by the thing that opens it.
///
/// Applying without a dry run is the right default *here* and not on the command line: the steps
/// are the ones this binary was built with, they are idempotent, and a definition stamped ahead of
/// this binary is still refused rather than rewritten. A boot that changes nothing — every store
/// that is already current — returns an empty [`Applied`] and should not be logged as an event.
///
/// # Errors
/// As [`apply`]. A caller on the boot path should log the failure and carry on: an unmigrated store
/// is a store that reads oddly, and a panel that refuses to start is one that cannot be used to fix
/// it.
pub fn on_boot(agents: &crate::Agents) -> Result<Applied> {
    let registry = LlmBackends::with_config(agents.config().clone());
    apply(agents, &registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentManifest, RawAgentArguments};
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

    /// The runtime *the file* carries, which since v3 is not the one `get` answers with.
    fn stored_runtime(agents: &crate::Agents, name: &str) -> Option<Backend> {
        agents
            .raw_manifest(name)
            .expect("raw")
            .expect("present")
            .backend
    }

    /// Put a backend in the store, so a chain naming it has a head to take a runtime from.
    fn backend(agents: &crate::Agents, id: &str, runtime: Backend) -> LlmBackends {
        let registry = LlmBackends::with_config(agents.config().clone());
        registry
            .save(
                id,
                crate::llm::LlmBackendManifest {
                    label: id.to_string(),
                    runtime,
                    model: "claude-opus-5".to_string(),
                    ..Default::default()
                },
            )
            .expect("save backend");
        registry
    }

    fn legacy(model: &str) -> AgentManifest<RawAgentArguments> {
        let mut arguments = RawAgentArguments::new();
        arguments.insert("model".to_string(), serde_json::json!(model));
        AgentManifest {
            backend: Some(Backend::HarnessClaudeSdk),
            arguments,
            ..Default::default()
        }
    }

    #[test]
    fn a_file_with_no_version_line_is_unversioned_rather_than_version_one() {
        let manifest = legacy("opus");
        assert_eq!(manifest.version, UNVERSIONED, "nothing writes a zero");
        assert_eq!(STEPS[0].from, UNVERSIONED, "and the chain starts there");
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
        assert_eq!(saved.manifest.version, UNVERSIONED, "still unstamped");
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
        assert_eq!(plan.pending[0].from, UNVERSIONED);
        assert_eq!(
            plan.pending[0].steps,
            vec![
                "stamp".to_string(),
                "llm-backends".to_string(),
                "runtime".to_string()
            ],
            "an unstamped file walks the chain from nothing"
        );
        assert_eq!(plan.current, 0);
        assert!(plan.ahead.is_empty());
    }

    /// The other half of the same rule: a definition that already says what it is only gets the
    /// steps above it, not the whole chain again.
    #[test]
    fn a_stamped_agent_only_gets_the_steps_above_it() {
        let agents = scratch("a_stamped_agent_only_gets_the_steps_above_it");
        agents
            .save_migrated("old", legacy("opus").to_stored().expect("stored"), 1)
            .expect("seed");

        let plan = plan(&agents).expect("plan");
        assert_eq!(plan.pending[0].from, 1);
        assert_eq!(
            plan.pending[0].steps,
            vec!["llm-backends".to_string(), "runtime".to_string()]
        );
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
        assert_eq!(
            applied.steps,
            vec!["stamp", "llm-backends", "runtime"],
            "from nothing, one version at a time"
        );

        let saved = agents.get("old").expect("get").expect("present").manifest;
        assert_eq!(saved.version, MANIFEST_VERSION);
        assert_eq!(saved.backends.len(), 1, "it now names a backend");
        assert!(!saved.arguments.contains_key("model"), "and not a model");
        assert_eq!(
            saved.backend,
            Some(Backend::HarnessClaudeSdk),
            "and reads back with the runtime of the backend it now names"
        );
        assert!(
            stored_runtime(&agents, "old").is_none(),
            "which the file itself no longer says"
        );
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
        let registry = backend(&agents, "anthropic", Backend::HarnessClaudeSdk);
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
        assert_eq!(
            registry.list().expect("list").len(),
            1,
            "and no backend was invented for an agent that already had one"
        );
    }

    /// The whole of the third step, on the shape it was written for: the runtime leaves the file,
    /// and reading the agent back answers with the one on the backend it starts on.
    #[test]
    fn the_runtime_leaves_a_chained_agent_and_comes_back_from_its_first_backend() {
        let agents = scratch("the_runtime_leaves_a_chained_agent_and_comes_back_from_its_first_backend");
        let registry = backend(&agents, "anthropic", Backend::HarnessClaudeSdk);
        let mut manifest = legacy("opus").to_stored().expect("stored");
        manifest.arguments.remove("model");
        manifest.backends = vec![AgentBackendEntry::new("anthropic")];
        agents.save_migrated("chained", manifest, 2).expect("seed");
        assert_eq!(stored_runtime(&agents, "chained"), Some(Backend::HarnessClaudeSdk));

        let applied = apply(&agents, &registry).expect("apply");
        assert_eq!(applied.steps, vec!["runtime"]);
        assert_eq!(applied.agents, 1);
        assert!(applied.held.is_empty());

        assert_eq!(stored_runtime(&agents, "chained"), None, "off the file");
        let saved = agents.get("chained").expect("get").expect("present").manifest;
        assert_eq!(saved.version, 3);
        assert_eq!(saved.backend, Some(Backend::HarnessClaudeSdk), "and back on the read");
    }

    /// An agent that asks no LLM backend anything — a `pty:claude` one — has nothing to derive a
    /// runtime from, so v3 stamps it and leaves the field exactly where it was.
    #[test]
    fn an_agent_with_no_chain_keeps_the_runtime_it_declares() {
        let agents = scratch("an_agent_with_no_chain_keeps_the_runtime_it_declares");
        let registry = LlmBackends::with_config(agents.config().clone());
        let mut manifest = legacy("opus").to_stored().expect("stored");
        manifest.arguments.remove("model");
        manifest.backend = Some(Backend::PtyClaude);
        agents.save_migrated("driver", manifest, 2).expect("seed");

        let applied = apply(&agents, &registry).expect("apply");
        assert_eq!(applied.agents, 1);
        assert!(applied.held.is_empty(), "there is nothing to decide");

        let saved = agents.get("driver").expect("get").expect("present").manifest;
        assert_eq!(saved.version, MANIFEST_VERSION);
        assert_eq!(stored_runtime(&agents, "driver"), Some(Backend::PtyClaude));
        assert_eq!(saved.backend, Some(Backend::PtyClaude));
    }

    /// The one case the step will not decide: the file says one runner and the head of the chain
    /// says another. Dropping the field would silently move the agent, so it is left at v2 and
    /// named.
    #[test]
    fn an_agent_whose_runtime_disagrees_with_its_first_backend_is_held_back() {
        let agents = scratch("an_agent_whose_runtime_disagrees_with_its_first_backend_is_held_back");
        let registry = backend(&agents, "glm", Backend::HarnessAdi);
        let mut manifest = legacy("opus").to_stored().expect("stored");
        manifest.arguments.remove("model");
        // Written as harness:claude-sdk by `legacy`, but it starts on a harness:adi backend.
        manifest.backends = vec![AgentBackendEntry::new("glm")];
        agents.save_migrated("split", manifest, 2).expect("seed");

        let applied = apply(&agents, &registry).expect("apply");
        assert_eq!(applied.agents, 0, "nothing was moved");
        let held = applied.held.get("split").expect("named");
        assert!(held.contains("harness:claude-sdk"), "{held}");
        assert!(held.contains("harness:adi"), "{held}");

        let saved = agents.get("split").expect("get").expect("present").manifest;
        assert_eq!(saved.version, 2, "left where it was, for a human to settle");
        assert_eq!(stored_runtime(&agents, "split"), Some(Backend::HarnessClaudeSdk));
    }

    /// A chain whose first row names a backend that was deleted: the chain cannot say what the
    /// agent runs on, so the file is not asked to stop saying it either.
    #[test]
    fn an_agent_whose_first_backend_is_gone_is_held_back() {
        let agents = scratch("an_agent_whose_first_backend_is_gone_is_held_back");
        let registry = LlmBackends::with_config(agents.config().clone());
        let mut manifest = legacy("opus").to_stored().expect("stored");
        manifest.arguments.remove("model");
        manifest.backends = vec![AgentBackendEntry::new("deleted")];
        agents.save_migrated("orphan", manifest, 2).expect("seed");

        let applied = apply(&agents, &registry).expect("apply");
        assert_eq!(applied.agents, 0);
        assert!(applied.held.contains_key("orphan"));
        let saved = agents.get("orphan").expect("get").expect("present").manifest;
        assert_eq!(saved.version, 2);
        assert_eq!(stored_runtime(&agents, "orphan"), Some(Backend::HarnessClaudeSdk));
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

        // Carried onto the result too, because the boot path never asks for a plan: an old binary
        // opening a store a newer one has migrated has to be able to say so from `apply` alone.
        let applied = apply(&agents, &registry).expect("apply");
        assert_eq!(applied.ahead.get("from-the-future"), Some(&(MANIFEST_VERSION + 7)));
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

        assert_eq!(store_version(&agents).expect("mixed"), UNVERSIONED);
    }

    /// What `adi-app` does as it opens the store: no plan, no flag, and nothing left behind.
    #[test]
    fn booting_brings_the_store_forward_and_a_second_boot_is_quiet() {
        let agents = scratch("booting_brings_the_store_forward_and_a_second_boot_is_quiet");
        agents
            .save_migrated("old", legacy("opus").to_stored().expect("stored"), 0)
            .expect("seed");

        let applied = on_boot(&agents).expect("boot");
        assert_eq!(applied.agents, 1);
        assert_eq!(
            agents.get("old").expect("get").expect("present").manifest.version,
            MANIFEST_VERSION
        );
        assert_eq!(on_boot(&agents).expect("second boot").agents, 0);
    }

    #[test]
    fn every_step_moves_exactly_one_version_and_they_join_up() {
        let mut expected = UNVERSIONED;
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
