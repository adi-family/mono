use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::backend::Backend;
use crate::error::{Error, Result};

pub type RawAgentArguments = BTreeMap<String, serde_json::Value>;

/// The key every backend's arguments spell their system prompt under.
const SYSTEM_PROMPT: &str = "system_prompt";

pub type StoredAgentManifest = AgentManifest<RawAgentArguments>;

pub type StoredAgent = Agent<RawAgentArguments>;

/// A reference to one secret attached to an agent — its scope (`project`, or `None` for a global
/// secret) and key `name`. At launch, exactly the secrets in an agent's attachment list are
/// decrypted and exported into the run's environment under their literal `name`s: an explicit
/// **allowlist**, not the whole scope. Serialized as a TOML array-of-tables (`[[secrets]]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SecretAttachment {
    /// The scope of the attached secret: a project id, or absent/`None` for a global secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The secret's key name — also the env-var name it injects into the run as.
    pub name: String,
}

/// An agent definition with backend-specific arguments.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, bound(deserialize = "Args: Deserialize<'de> + Default"))]
pub struct AgentManifest<Args> {
    pub backend: Backend,
    pub arguments: Args,
    pub tags: Vec<String>,
    pub starred: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The ids of the adi **tools** enabled for this agent (its per-tool checkboxes). Each becomes
    /// a shim in the agent's own `.bin` (see `adi_tools::Tools::sync_agent_bin`), materialized on
    /// its PATH at launch. Empty = no tools. Named `bin_tools` to stay distinct from the LLM
    /// `--allowed-tools` (which lives in `arguments.tools`); these are ADI CLIs the agent can run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bin_tools: Vec<String>,
    /// The knowledge bases this agent works with, written the way `adi-mono knowledge` writes
    /// them: `global/runbooks`, `project:acme/notes`, `agent:reviewer/memory`.
    ///
    /// A **wish list**, not a grant. What an agent may actually reach is decided by the three
    /// isolation levels at read time (see `adi_knowledge::Reader::access`), so naming another
    /// project's base here gets it dropped, and naming another agent's gets it read-only. Empty
    /// = whatever the agent finds for itself; nothing here is required for it to run.
    ///
    /// Stays ahead of [`secrets`](Self::secrets) in declaration order because the registry is
    /// TOML, where a plain array cannot follow an array-of-tables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<String>,
    /// Whether this agent keeps a memory of its own — the `agent:<name>/memory` base, which it
    /// alone writes and every other agent may read.
    ///
    /// Off by default, deliberately: an agent that records what it learns is a different thing
    /// from one that does not, and that should be a decision somebody made rather than one the
    /// default made for them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub memory: bool,
    /// The secrets attached to this agent (its per-secret checkboxes). At launch, exactly these
    /// are decrypted and injected into the run's environment under their literal names — an
    /// explicit allowlist, so nothing is inherited from a scope just for existing. Empty = the
    /// run gets no secrets. See [`SecretAttachment`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretAttachment>,
    /// Extra directories to put on the run's `PATH` — the agent's answer to "this project needs a
    /// toolchain the machine's default `PATH` doesn't point at" (a pinned nvm node, say). Each
    /// entry may lead with `~` or `$HOME`. They land right after the agent's `.bin`, so its own
    /// tools still win, and ahead of every standard dir, so they beat the system copy of the same
    /// binary. Stored as a TOML array: `path = ["$HOME/.nvm/versions/node/v22.14.0/bin"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    /// Extra environment variables for the run, injected under their literal names — the plain
    /// `KEY = "value"` half of the same idea. Applied after the attached secrets, so an entry here
    /// wins over a secret of the same name. `PATH` is the one key that cannot be set here: it is
    /// built from [`path`](Self::path) and applied last, so no declaration can strand a run
    /// without its tools. Stored as a TOML table (`[env]`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Whether this agent runs with nobody watching — a trigger's agent, a scheduled sweep, a run
    /// nobody will see until morning.
    ///
    /// It changes exactly one thing: the [`Ask`](crate::backends::harness) tool refuses, telling
    /// the run to decide for itself and say what it assumed. A question nobody is going to answer
    /// is worse than a judgement call, because the judgement call at least ships — and an
    /// unattended run that asks is a run that has quietly stopped without failing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unattended: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

impl<Args> adi_config::Timestamped for AgentManifest<Args> {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

/// A manifest paired with its filename-derived name.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agent<Args> {
    pub name: String,
    pub manifest: AgentManifest<Args>,
}

impl<Args> AgentManifest<Args> {
    /// The executor (`pty` / `process` / `harness`) — the part before the `:` in
    /// [`Self::backend`]; empty string if the backend has no `executor:` prefix. Drives how the
    /// agent runs and which params apply.
    #[must_use]
    pub fn executor(&self) -> &str {
        self.backend.executor()
    }

    /// Build a manifest that carries this one's metadata — `backend`, `tags`, `starred`,
    /// `project`, and both timestamps — but swaps in a freshly derived `arguments` payload. The
    /// single place that field list lives, so the encode/decode paths below don't each respell it.
    /// Clones the carried fields (all cheap) and leaves `self`'s own `arguments` untouched.
    fn rewrap<T>(&self, arguments: T) -> AgentManifest<T> {
        AgentManifest {
            backend: self.backend.clone(),
            arguments,
            tags: self.tags.clone(),
            starred: self.starred,
            project: self.project.clone(),
            bin_tools: self.bin_tools.clone(),
            knowledge: self.knowledge.clone(),
            memory: self.memory,
            secrets: self.secrets.clone(),
            path: self.path.clone(),
            env: self.env.clone(),
            unattended: self.unattended,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

impl<Args: Serialize> AgentManifest<Args> {
    /// # Errors
    /// Returns [`Error::Arguments`] when `Args` cannot be stored as a TOML object.
    pub fn to_stored(&self) -> Result<StoredAgentManifest> {
        Ok(self.rewrap(encode_arguments(&self.arguments)?))
    }
}

impl AgentManifest<RawAgentArguments> {
    /// # Errors
    /// Returns [`Error::Arguments`] when the stored object does not match `Args`.
    pub fn typed_arguments<Args: DeserializeOwned>(&self) -> Result<Args> {
        decode_arguments(self.arguments.clone())
    }

    /// The stored arguments as one JSON object — the engine configuration a
    /// [`RunSpec`](crate::runner::RunSpec) carries, uninterpreted.
    ///
    /// The map *is* the document; this only changes its container, so each runner can deserialize
    /// its own typed struct out of it without any layer above holding a table of argument types.
    pub(crate) fn arguments_value(&self) -> serde_json::Value {
        serde_json::Value::Object(self.arguments.clone().into_iter().collect())
    }

    /// The agent's own system prompt, exactly as it was written, or `None` when it has none.
    ///
    /// Read off the raw arguments rather than a decoded struct because every backend spells it under
    /// the same key, which is what lets one reader answer for all of them. Nothing above the runner
    /// writes into it.
    pub(crate) fn system_prompt(&self) -> Option<String> {
        self.arguments
            .get(SYSTEM_PROMPT)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .map(ToString::to_string)
    }

    /// # Errors
    /// Returns [`Error::Arguments`] when the stored object does not match `Args`.
    pub fn into_typed<Args: DeserializeOwned>(self) -> Result<AgentManifest<Args>> {
        let arguments = self.typed_arguments()?;
        Ok(self.rewrap(arguments))
    }
}

impl Agent<RawAgentArguments> {
    /// # Errors
    /// Returns [`Error::Arguments`] when the stored object does not match `Args`.
    pub fn into_typed<Args: DeserializeOwned>(self) -> Result<Agent<Args>> {
        Ok(Agent {
            name: self.name,
            manifest: self.manifest.into_typed()?,
        })
    }
}

fn encode_arguments<Args: Serialize>(arguments: &Args) -> Result<RawAgentArguments> {
    let value = serde_json::to_value(arguments).map_err(|e| Error::Arguments(e.to_string()))?;
    if contains_json_null(&value) {
        return Err(Error::Arguments(
            "arguments cannot contain null because the registry is stored as TOML".into(),
        ));
    }
    let serde_json::Value::Object(arguments) = value else {
        return Err(Error::Arguments(
            "backend arguments must serialize as an object".into(),
        ));
    };
    Ok(arguments.into_iter().collect())
}

fn decode_arguments<Args: DeserializeOwned>(arguments: RawAgentArguments) -> Result<Args> {
    let arguments = serde_json::Map::from_iter(arguments);
    serde_json::from_value(serde_json::Value::Object(arguments))
        .map_err(|e| Error::Arguments(e.to_string()))
}

/// Whether `value` contains a JSON `null` anywhere in its tree. Callers use this to reject
/// arguments before they reach the manifest store, because TOML has no `null` and the value
/// would be silently dropped on serialization.
#[must_use]
pub fn contains_json_null(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Array(values) => values.iter().any(contains_json_null),
        serde_json::Value::Object(values) => values.values().any(contains_json_null),
        _ => false,
    }
}

/// Validate an agent name before it is joined onto the store path as `<name>.toml`, mapping a
/// rejection onto [`Error::InvalidName`].
pub(crate) fn validate_name(name: &str) -> Result<()> {
    adi_config::validate_name(name, Error::InvalidName)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Deserialize)]
    struct SampleArguments {
        system_prompt: String,
        tools: String,
        model: String,
        permission_mode: String,
        temperature: f64,
        max_turns: u64,
        provider: String,
    }

    #[test]
    fn executor_is_the_prefix_before_the_colon() {
        for (backend, executor) in [
            ("pty:claude", "pty"),
            ("process:codex", "process"),
            ("harness:claude-sdk", "harness"),
            ("weird", ""),
        ] {
            let manifest = AgentManifest::<()> {
                backend: backend.into(),
                ..Default::default()
            };
            assert_eq!(manifest.executor(), executor);
        }
    }

    #[test]
    fn missing_fields_deserialize_from_the_manifest_default() {
        let manifest: StoredAgentManifest = serde_json::from_str("{}").expect("empty manifest");
        assert_eq!(manifest, StoredAgentManifest::default());
    }

    #[test]
    fn arguments_object_decodes_into_typed_and_round_trips() {
        let manifest: StoredAgentManifest = serde_json::from_str(
            r#"{
                "backend":"process:claude",
                "arguments":{
                    "system_prompt":"Solve it",
                    "tools":"tasks,projects",
                    "model":"opus",
                    "permission_mode":"plan",
                    "temperature":0.2,
                    "max_turns":12,
                    "provider":"anthropic"
                }
            }"#,
        )
        .expect("manifest");

        let typed = manifest
            .clone()
            .into_typed::<SampleArguments>()
            .expect("typed arguments");
        assert_eq!(typed.arguments.system_prompt, "Solve it");
        assert_eq!(typed.arguments.tools, "tasks,projects");
        assert_eq!(typed.arguments.model, "opus");
        assert_eq!(typed.arguments.permission_mode, "plan");
        assert!((typed.arguments.temperature - 0.2).abs() < f64::EPSILON);
        assert_eq!(typed.arguments.max_turns, 12);
        assert_eq!(typed.arguments.provider, "anthropic");

        // The stored shape keeps every backend param under `arguments`, never at the top level.
        let serialized = serde_json::to_value(&manifest).expect("serialize");
        assert_eq!(serialized["arguments"]["system_prompt"], "Solve it");
        for top_level in ["system_prompt", "tools", "model", "max_turns"] {
            assert!(serialized.get(top_level).is_none(), "top-level {top_level}");
        }
    }

    /// `rewrap` is respelled once per field, so a field added to the manifest and forgotten here
    /// is silently dropped on every encode/decode round trip — the agent keeps its knowledge
    /// bases until something saves it, and then quietly doesn't.
    #[test]
    fn the_knowledge_fields_survive_the_arguments_round_trip() {
        let mut manifest = StoredAgentManifest {
            backend: "harness:adi".into(),
            knowledge: vec!["global/runbooks".into(), "agent:reviewer/memory".into()],
            memory: true,
            ..Default::default()
        };
        manifest
            .arguments
            .insert("system_prompt".into(), "Solve it".into());

        let stored = manifest.to_stored().expect("to stored");
        assert_eq!(stored.knowledge, manifest.knowledge);
        assert!(stored.memory);

        let back: StoredAgentManifest = stored.into_typed().expect("back to typed");
        assert_eq!(back.knowledge, manifest.knowledge);
        assert!(back.memory);
        assert_eq!(back.arguments, manifest.arguments);
    }

    /// The registry is TOML, where a plain array may not follow an array-of-tables — so
    /// `knowledge` has to be declared ahead of `secrets`. Serializing a manifest carrying both
    /// is what catches a later reorder.
    #[test]
    fn a_manifest_with_knowledge_and_secrets_still_encodes_as_toml() {
        let manifest = StoredAgentManifest {
            backend: "harness:adi".into(),
            knowledge: vec!["global/runbooks".into()],
            memory: true,
            secrets: vec![SecretAttachment {
                project: None,
                name: "API_KEY".into(),
            }],
            ..Default::default()
        };
        let text = toml::to_string_pretty(&manifest).expect("toml");
        let back: StoredAgentManifest = toml::from_str(&text).expect("parse back");
        assert_eq!(back.knowledge, manifest.knowledge);
        assert!(back.memory);
        assert_eq!(back.secrets, manifest.secrets);
    }

    /// Both fields are omit-when-default, so every agent definition written before knowledge
    /// existed still loads — and does not grow two lines of noise when saved again.
    #[test]
    fn an_agent_definition_from_before_knowledge_still_loads() {
        let older = "backend = \"harness:adi\"\nstarred = false\ncreated_at = 1\nupdated_at = 2\n";
        let manifest: StoredAgentManifest = toml::from_str(older).expect("parse");
        assert!(manifest.knowledge.is_empty());
        assert!(!manifest.memory);

        let text = toml::to_string_pretty(&manifest).expect("toml");
        assert!(!text.contains("knowledge"), "{text}");
        assert!(!text.contains("memory"), "{text}");
    }

    #[test]
    fn valid_and_invalid_names() {
        for name in ["athz-solver", "planner", "agent_2", "a.b"] {
            assert!(validate_name(name).is_ok(), "{name} should be valid");
        }
        for name in ["", ".", "..", "a/b", "a\\b", "with space"] {
            assert!(
                matches!(validate_name(name), Err(Error::InvalidName(_))),
                "{name:?} should be rejected"
            );
        }
    }
}
