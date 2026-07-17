use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::backend::Backend;
use crate::error::{Error, Result};

pub type RawAgentArguments = BTreeMap<String, serde_json::Value>;

pub type StoredAgentManifest = AgentManifest<RawAgentArguments>;

pub type StoredAgent = Agent<RawAgentArguments>;

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
    pub created_at: u64,
    pub updated_at: u64,
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
}

impl<Args: Serialize> AgentManifest<Args> {
    /// # Errors
    /// Returns [`Error::Arguments`] when `Args` cannot be stored as a TOML object.
    pub fn to_stored(&self) -> Result<StoredAgentManifest> {
        Ok(AgentManifest {
            backend: self.backend.clone(),
            arguments: encode_arguments(&self.arguments)?,
            tags: self.tags.clone(),
            starred: self.starred,
            project: self.project.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
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
        Ok(AgentManifest {
            backend: self.backend,
            arguments: decode_arguments(self.arguments)?,
            tags: self.tags,
            starred: self.starred,
            project: self.project,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
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

#[must_use]
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate an agent name: a single, filesystem-safe path segment. This is a security boundary —
/// names arrive from the CLI and the HTTP API and are joined onto the store path as
/// `<name>.toml`, so anything with a separator or `.`/`..` must be rejected.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_string()))
    }
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
