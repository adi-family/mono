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
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
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

fn contains_json_null(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Array(values) => values.iter().any(contains_json_null),
        serde_json::Value::Object(values) => values.values().any(contains_json_null),
        _ => false,
    }
}

/// The pre-`arguments` storage shape. Deserialization folds these legacy backend fields into
/// `arguments`, while serialization only ever writes the compact ADI-owned manifest shape.
#[derive(Default, Deserialize)]
#[serde(default)]
struct SerializedAgentManifest {
    backend: String,
    arguments: BTreeMap<String, serde_json::Value>,
    tags: Vec<String>,
    starred: bool,
    project: Option<String>,
    created_at: u64,
    updated_at: u64,
    system_prompt: String,
    tools: String,
    model: Option<String>,
    permission_mode: Option<String>,
    temperature: Option<f64>,
    max_turns: Option<u32>,
    extra: BTreeMap<String, String>,
}

impl<'de, Args: DeserializeOwned> Deserialize<'de> for AgentManifest<Args> {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let stored = SerializedAgentManifest::deserialize(deserializer)?;
        let mut arguments = stored.arguments;

        let mut insert = |name: &str, value: serde_json::Value| {
            arguments.entry(name.to_string()).or_insert(value);
        };
        if !stored.system_prompt.is_empty() {
            insert("system_prompt", stored.system_prompt.into());
        }
        if !stored.tools.is_empty() {
            insert("tools", stored.tools.into());
        }
        if let Some(value) = stored.model {
            insert("model", value.into());
        }
        if let Some(value) = stored.permission_mode {
            insert("permission_mode", value.into());
        }
        if let Some(value) = stored.temperature.and_then(serde_json::Number::from_f64) {
            insert("temperature", value.into());
        }
        if let Some(value) = stored.max_turns {
            insert("max_turns", value.into());
        }
        for (name, value) in stored.extra {
            insert(&name, value.into());
        }

        let arguments = decode_arguments(arguments).map_err(serde::de::Error::custom)?;

        Ok(Self {
            backend: Backend::from(stored.backend),
            arguments,
            tags: stored.tags,
            starred: stored.starred,
            project: stored.project,
            created_at: stored.created_at,
            updated_at: stored.updated_at,
        })
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
    struct LegacyArguments {
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
    fn legacy_backend_fields_migrate_into_arguments() {
        let manifest: StoredAgentManifest = serde_json::from_str(
            r#"{
                "backend":"process:claude",
                "system_prompt":"Solve it",
                "tools":"tasks,projects",
                "model":"opus",
                "permission_mode":"plan",
                "temperature":0.2,
                "max_turns":12,
                "extra":{"provider":"anthropic"}
            }"#,
        )
        .expect("legacy manifest");

        let typed = manifest
            .clone()
            .into_typed::<LegacyArguments>()
            .expect("typed legacy arguments");
        assert_eq!(typed.arguments.system_prompt, "Solve it");
        assert_eq!(typed.arguments.tools, "tasks,projects");
        assert_eq!(typed.arguments.model, "opus");
        assert_eq!(typed.arguments.permission_mode, "plan");
        assert!((typed.arguments.temperature - 0.2).abs() < f64::EPSILON);
        assert_eq!(typed.arguments.max_turns, 12);
        assert_eq!(typed.arguments.provider, "anthropic");

        let serialized = serde_json::to_value(manifest).expect("serialize");
        for legacy in [
            "system_prompt",
            "tools",
            "model",
            "permission_mode",
            "temperature",
            "max_turns",
            "extra",
        ] {
            assert!(serialized.get(legacy).is_none(), "legacy field {legacy}");
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
