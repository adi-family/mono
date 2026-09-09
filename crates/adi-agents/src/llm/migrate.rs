//! The one-time upgrade: model configuration leaves the agent and becomes a backend.
//!
//! Before this, every agent carried its own model, its own login and its own dials, and an agent
//! that ran out of quota was simply stuck. Now a model lives in exactly one place — a backend under
//! a name — and an agent names backends in order. This module moves the former into the latter,
//! **1:1**: each distinct configuration found on an agent becomes one backend, and that backend
//! becomes row 1 of the agent that had it. Two agents configured identically land on the same
//! backend; two configured differently land on two, even where a human would later merge them.
//!
//! It is deliberately not clever. Collapsing `adi-agent`, `adi-agent-glm` and `adi-agent-kimi` into
//! one agent listing three backends is the *point* of the feature, but it is a judgement about
//! which agents are the same agent, and nothing here can make it. The migration gets every agent
//! onto the new shape without changing what any of them does today; the collapsing is done by hand
//! afterwards, by somebody who knows which three names meant one job.
//!
//! What moves and what stays is the same split the whole design rests on. The model, the four
//! credential fields and the sampling dials describe *the model*, so they go to the backend. The
//! prompt, the tool grant, the permission mode, the turn cap, the working directory and the
//! executor's own switches describe *the agent* or *the runner*, so they stay exactly where they
//! are. An agent's runtime is copied onto its backend and left on the agent too: a backend row sets
//! the runtime when one is resolved, and an agent that ends up listing no backends must still run.
//!
//! Running it twice is safe. An agent that already lists a backend is left alone, and a
//! configuration that matches a backend already in the store reuses it instead of writing a second.

use std::collections::{BTreeMap, BTreeSet};

use crate::agent::StoredAgent;
use crate::error::Result;
use crate::llm::backend::{CREDENTIAL_KEYS, LlmBackendManifest, LlmBackends, MODEL_KEY};
use crate::llm::chain::AgentBackendEntry;

/// The argument keys that describe how the model *thinks* — everything the runners accept that is
/// neither the model name, nor the login, nor a fact about the agent or its executor.
///
/// Kept as a list rather than derived from the argument structs because the structs do not mark
/// which of their fields are dials, and guessing wrong in either direction is expensive: a dial
/// left behind is a setting that silently stops applying, and an executor switch dragged onto a
/// backend is a setting that starts applying to every agent that names it.
pub const DIAL_KEYS: [&str; 12] = [
    "effort",
    "fallback_model",
    "max_budget_usd",
    "max_tokens",
    "reasoning_effort",
    "seed",
    "stop",
    "temperature",
    "thinking",
    "thinking_budget",
    "top_k",
    "top_p",
];

/// One agent's move: where its model configuration goes, and what leaves the agent to get there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub agent: String,
    /// The backend id this agent's list will name.
    pub backend: String,
    /// Whether that backend is written by this migration, or was already in the store.
    pub created: bool,
    /// The argument keys that leave the agent, in the order they are printed.
    pub moved: Vec<String>,
}

/// Why an agent is not moving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub agent: String,
    pub why: String,
}

/// What the migration would do, computed without writing anything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// One entry per agent that will gain a list, in name order.
    pub moves: Vec<Move>,
    /// The backends this migration will write, by id. Backends it merely reuses are not here.
    pub backends: BTreeMap<String, LlmBackendManifest>,
    /// The agents it will not touch, and why — printed, because "nothing happened" is an answer
    /// somebody needs the reason for.
    pub skipped: Vec<Skip>,
}

impl Plan {
    /// Whether there is anything to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }
}

/// Work out what the upgrade would do to every agent in the store.
///
/// # Errors
/// Everything [`crate::Agents::list`] and [`LlmBackends::list`] can return — a manifest that cannot
/// be read is a manifest this must not plan around.
pub fn plan(agents: &crate::Agents, registry: &LlmBackends) -> Result<Plan> {
    let existing = registry.list()?;
    // Two indexes over what is already stored: by fingerprint, so an identical configuration reuses
    // a backend somebody already made rather than growing a near-duplicate; and by id, so a name
    // this migration invents never lands on one that is taken.
    let mut by_print: BTreeMap<String, String> = BTreeMap::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for backend in existing {
        by_print
            .entry(fingerprint(&backend.manifest))
            .or_insert_with(|| backend.id.clone());
        taken.insert(backend.id);
    }

    let mut plan = Plan::default();
    for agent in agents.list()? {
        if !agent.manifest.backends.is_empty() {
            plan.skipped.push(Skip {
                agent: agent.name.clone(),
                why: format!(
                    "already lists {}",
                    agent
                        .manifest
                        .backends
                        .iter()
                        .map(|row| row.backend.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
            continue;
        }
        let (manifest, moved) = extract(&agent);
        let print = fingerprint(&manifest);
        let (id, created) = match by_print.get(&print) {
            Some(id) => (id.clone(), false),
            None => {
                let id = unique_id(&suggest_id(&manifest), &taken);
                taken.insert(id.clone());
                by_print.insert(print, id.clone());
                plan.backends.insert(id.clone(), manifest);
                (id, true)
            }
        };
        plan.moves.push(Move {
            agent: agent.name,
            backend: id,
            created,
            moved,
        });
    }
    Ok(plan)
}

/// Carry a plan out: write the new backends, then rewrite each agent onto its one-row list.
///
/// The backends are written **first**, so that an agent is never saved naming a definition that is
/// not there yet — a half-finished run of this would leave chains that resolve to nothing.
///
/// # Errors
/// [`crate::error::Error::Config`] when a definition or a manifest cannot be written, plus anything
/// [`crate::Agents::get`] can return. Stops at the first failure with the earlier writes kept: they
/// are each complete on their own, and re-running finishes the rest.
pub fn apply(agents: &crate::Agents, registry: &LlmBackends, plan: &Plan) -> Result<usize> {
    for (id, manifest) in &plan.backends {
        registry.save(id, manifest.clone())?;
    }
    let mut changed = 0;
    for mv in &plan.moves {
        let Some(mut agent) = agents.get(&mv.agent)? else {
            // Deleted between planning and applying. Not an error: the plan was a description of
            // the store as it was, and an agent that no longer exists needs no migrating.
            continue;
        };
        for key in &mv.moved {
            agent.manifest.arguments.remove(key);
        }
        agent.manifest.backends = vec![AgentBackendEntry {
            backend: mv.backend.clone(),
            overrides: BTreeMap::new(),
        }];
        agents.save(&mv.agent, agent.manifest)?;
        changed += 1;
    }
    Ok(changed)
}

/// Read an agent's model configuration out as a backend, and say which keys it came from.
///
/// The runtime is copied rather than moved: a resolved row sets the runtime it runs on, but an
/// agent still needs one of its own for the runs that resolve no row at all.
fn extract(agent: &StoredAgent) -> (LlmBackendManifest, Vec<String>) {
    let arguments = &agent.manifest.arguments;
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    };
    let mut moved = Vec::new();
    let mut note = |key: &str| moved.push(key.to_string());

    let model = text(MODEL_KEY).unwrap_or_default();
    if !model.is_empty() {
        note(MODEL_KEY);
    }
    let mut credential: [Option<String>; 4] = [None, None, None, None];
    for (slot, key) in credential.iter_mut().zip(CREDENTIAL_KEYS) {
        *slot = text(key);
        if slot.is_some() {
            note(key);
        }
    }
    let mut params = BTreeMap::new();
    for key in DIAL_KEYS {
        if let Some(value) = arguments.get(key) {
            params.insert(key.to_string(), value.clone());
            note(key);
        }
    }
    let [settings, provider, base_url, api_key_env] = credential;
    (
        LlmBackendManifest {
            label: String::new(),
            runtime: agent.manifest.runtime().clone(),
            model,
            // Nothing on an agent ever recorded a context window, so there is none to carry. Left
            // at zero, which means *unknown* and refuses no switch — the honest answer, and one the
            // operator fills in per backend once, rather than one this guesses per agent.
            context_tokens: 0,
            settings,
            provider,
            base_url,
            api_key_env,
            params,
            // Limit rules and the probe are new behaviour, not old configuration, so there is
            // nothing to migrate into them: a migrated backend never reroutes until somebody
            // writes a rule for it, and that is the safe direction.
            limit_rules: Vec::new(),
            probe: None,
            created_at: 0,
            updated_at: 0,
        },
        moved,
    )
}

/// A canonical string standing for "the same configuration", so two agents set up identically land
/// on one backend. Every field that would make the backends behave differently is in it — and the
/// timestamps and the label, which would not, are not.
fn fingerprint(manifest: &LlmBackendManifest) -> String {
    let params = serde_json::to_string(&manifest.params).unwrap_or_default();
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        manifest.runtime,
        manifest.model,
        manifest.settings.as_deref().unwrap_or_default(),
        manifest.provider.as_deref().unwrap_or_default(),
        manifest.base_url.as_deref().unwrap_or_default(),
        manifest.api_key_env.as_deref().unwrap_or_default(),
        params,
    )
}

/// A readable name for a configuration, from the most specific thing that names it.
///
/// The settings file is tried first for the same reason the credential is derived from it first: it
/// is what a person actually chose, and `settings.glm.json` is a name somebody will recognise where
/// `pty-claude-2` is not. Only when nothing names a login does this fall back to the runtime, which
/// is the honest name for "whatever that CLI is logged in as".
fn suggest_id(manifest: &LlmBackendManifest) -> String {
    if let Some(settings) = manifest.settings.as_deref() {
        let stem = settings
            .rsplit('/')
            .next()
            .unwrap_or(settings)
            .trim_end_matches(".json");
        // `settings.glm` is the file; `glm` is what it is about.
        let stem = stem.strip_prefix("settings.").unwrap_or(stem);
        let stem = stem.strip_suffix(".settings").unwrap_or(stem);
        let slug = slug(stem);
        if !slug.is_empty() && slug != "settings" {
            return slug;
        }
    }
    if let Some(provider) = manifest.provider.as_deref() {
        let slug = slug(provider);
        if !slug.is_empty() {
            return slug;
        }
    }
    if let Some(env) = manifest.api_key_env.as_deref() {
        let slug = slug(env.trim_end_matches("_API_KEY"));
        if !slug.is_empty() {
            return slug;
        }
    }
    slug(&manifest.runtime.to_string())
}

/// A name into a filename: lowercase, one dash between runs of anything else, no dash at either
/// end. Empty in, empty out — the caller decides what to do about that.
fn slug(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The suggested name, or the first `-2`, `-3`, … that nobody has. A collision here means two
/// genuinely different configurations wanted the same name — two models on one subscription, say —
/// and a number is a better answer than mangling one of them into a name it does not have.
fn unique_id(base: &str, taken: &BTreeSet<String>) -> String {
    let base = if base.is_empty() { "backend" } else { base };
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken.contains(id))
        .unwrap_or_else(|| base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentManifest, StoredAgentManifest};
    use crate::backend::Backend;
    use adi_config::Config;

    fn scratch(tag: &str) -> crate::Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-llm-migrate-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        crate::Agents::with_config(Config::with_root(root))
    }

    fn agent(backend: Backend, arguments: &[(&str, serde_json::Value)]) -> StoredAgentManifest {
        AgentManifest {
            backend: Some(backend),
            arguments: arguments
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
            ..AgentManifest::default()
        }
    }

    /// The shape of the whole upgrade: the model configuration is gone from the agent, it is on a
    /// backend, and the agent names that backend first.
    #[test]
    fn an_agents_model_configuration_becomes_its_first_backend() {
        let agents = scratch("one");
        let registry = LlmBackends::with_config(agents.config().clone());
        agents
            .save(
                "writer",
                agent(
                    Backend::HarnessClaudeSdk,
                    &[
                        ("model", "claude-opus-5".into()),
                        ("settings", "~/.claude/settings.glm.json".into()),
                        ("effort", "high".into()),
                        ("system_prompt", "be brief".into()),
                    ],
                ),
            )
            .expect("save");

        let plan = plan(&agents, &registry).expect("plan");
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].backend, "glm");
        assert!(plan.moves[0].created);
        assert_eq!(apply(&agents, &registry, &plan).expect("apply"), 1);

        let saved = agents.get("writer").expect("get").expect("writer");
        assert_eq!(saved.manifest.backends.len(), 1);
        assert_eq!(saved.manifest.backends[0].backend, "glm");
        for gone in ["model", "settings", "effort"] {
            assert!(!saved.manifest.arguments.contains_key(gone), "{gone}");
        }
        // The prompt is the agent's, not the model's, and must not have travelled.
        assert!(saved.manifest.arguments.contains_key("system_prompt"));

        let backend = registry.get("glm").expect("get").expect("glm");
        assert_eq!(backend.manifest.model, "claude-opus-5");
        assert_eq!(backend.manifest.settings.as_deref(), Some("~/.claude/settings.glm.json"));
        assert_eq!(backend.manifest.params.get("effort").and_then(|v| v.as_str()), Some("high"));
        // Copied, not moved: an agent resolving no row still has to run.
        assert_eq!(saved.manifest.backend, Some(Backend::HarnessClaudeSdk));
    }

    /// Two agents set up identically are one backend, not two — this is the whole reason the plan
    /// fingerprints a configuration rather than counting agents.
    #[test]
    fn identical_configurations_share_one_backend() {
        let agents = scratch("share");
        let registry = LlmBackends::with_config(agents.config().clone());
        for name in ["a", "b"] {
            agents
                .save(
                    name,
                    agent(Backend::HarnessAdi, &[("provider", "zai".into()), ("model", "glm-5.2".into())]),
                )
                .expect("save");
        }
        let plan = plan(&agents, &registry).expect("plan");
        assert_eq!(plan.backends.len(), 1, "{plan:?}");
        assert_eq!(plan.moves.len(), 2);
        assert_eq!(plan.moves[0].backend, plan.moves[1].backend);
        assert!(plan.moves[0].created != plan.moves[1].created);
    }

    /// Different models on one login are different backends, and the second gets a name of its own
    /// rather than overwriting the first.
    #[test]
    fn two_models_on_one_login_become_two_named_backends() {
        let agents = scratch("two");
        let registry = LlmBackends::with_config(agents.config().clone());
        for (name, model) in [("fast", "glm-5.2-air"), ("deep", "glm-5.2")] {
            agents
                .save(
                    name,
                    agent(Backend::HarnessAdi, &[("provider", "zai".into()), ("model", model.into())]),
                )
                .expect("save");
        }
        let plan = plan(&agents, &registry).expect("plan");
        assert_eq!(plan.backends.len(), 2, "{plan:?}");
        let ids: BTreeSet<&String> = plan.backends.keys().collect();
        assert!(ids.contains(&"zai".to_string()), "{ids:?}");
        assert!(ids.contains(&"zai-2".to_string()), "{ids:?}");
    }

    /// A configuration somebody already wrote a backend for is reused, not duplicated — which is
    /// also what makes a second run of the migration write nothing.
    #[test]
    fn an_existing_backend_is_reused_and_a_second_run_does_nothing() {
        let agents = scratch("idempotent");
        let registry = LlmBackends::with_config(agents.config().clone());
        registry
            .save(
                "house",
                LlmBackendManifest {
                    runtime: Backend::HarnessAdi,
                    model: "glm-5.2".into(),
                    provider: Some("zai".into()),
                    ..LlmBackendManifest::default()
                },
            )
            .expect("save backend");
        agents
            .save(
                "a",
                agent(Backend::HarnessAdi, &[("provider", "zai".into()), ("model", "glm-5.2".into())]),
            )
            .expect("save");

        let first = plan(&agents, &registry).expect("plan");
        assert!(first.backends.is_empty(), "{first:?}");
        assert_eq!(first.moves[0].backend, "house");
        assert!(!first.moves[0].created);
        apply(&agents, &registry, &first).expect("apply");

        let second = plan(&agents, &registry).expect("plan");
        assert!(second.is_empty(), "{second:?}");
        assert_eq!(second.skipped.len(), 1);
        assert!(second.skipped[0].why.contains("house"), "{second:?}");
    }

    /// An agent with nothing but a runtime still gets a backend: after the migration there is one
    /// place a model is configured, and "the CLI's own login" is a configuration like any other.
    #[test]
    fn an_agent_with_no_model_settings_still_gets_a_backend_named_for_its_runtime() {
        let agents = scratch("bare");
        let registry = LlmBackends::with_config(agents.config().clone());
        agents
            .save("bare", agent(Backend::PtyClaude, &[]))
            .expect("save");
        let plan = plan(&agents, &registry).expect("plan");
        assert_eq!(plan.moves[0].backend, "pty-claude");
        assert!(plan.moves[0].moved.is_empty());
    }

    /// The dividing line, stated as a test: a dial is the model's, an executor switch is not.
    #[test]
    fn executor_switches_stay_on_the_agent() {
        let agents = scratch("split");
        let registry = LlmBackends::with_config(agents.config().clone());
        agents
            .save(
                "coder",
                agent(
                    Backend::ProcessCodex,
                    &[
                        ("reasoning_effort", "high".into()),
                        ("sandbox", "workspace-write".into()),
                        ("approval", "on-request".into()),
                        ("working_dir", "/tmp".into()),
                    ],
                ),
            )
            .expect("save");
        let plan = plan(&agents, &registry).expect("plan");
        assert_eq!(plan.moves[0].moved, ["reasoning_effort"]);
        apply(&agents, &registry, &plan).expect("apply");
        let saved = agents.get("coder").expect("get").expect("coder");
        for kept in ["sandbox", "approval", "working_dir"] {
            assert!(saved.manifest.arguments.contains_key(kept), "{kept}");
        }
    }
}
