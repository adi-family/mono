use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::backend::Backend;
use crate::error::{Error, Result};

pub type RawAgentArguments = BTreeMap<String, serde_json::Value>;

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
    /// The secrets attached to this agent (its per-secret checkboxes). At launch, exactly these
    /// are decrypted and injected into the run's environment under their literal names — an
    /// explicit allowlist, so nothing is inherited from a scope just for existing. Empty = the
    /// run gets no secrets. See [`SecretAttachment`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretAttachment>,
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
    /// The executor (`tmux` / `process` / `harness`) — the part before the `:` in
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
            secrets: self.secrets.clone(),
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
            ("tmux:claude", "tmux"),
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
