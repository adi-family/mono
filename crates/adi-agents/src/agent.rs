//! The on-disk agent definition ([`AgentManifest`], serialized as `<name>.toml`) and the
//! name-attached view of a loaded agent ([`Agent`]).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A reusable, backend-agnostic agent definition — the stored spec from docs/adi-agents.md §5,
/// minus the orchestration/run machinery (which is future work). It says *what* an agent is
/// (which engine, which system prompt, which CLI commands, which knobs), not how to run it.
///
/// Unknown fields are ignored so the manifest can gain fields without breaking older stores.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AgentManifest {
    /// How and what runs the agent, as an `executor:what` string. The executor is the run
    /// mechanism, the suffix is the thing it runs: `tmux:claude` | `tmux:codex` (a vendor CLI in
    /// a tmux session), `process:claude` | `process:codex` (a vendor CLI as a detached headless
    /// subprocess), `harness:claude-sdk` | `harness:adi` (an agentic-loop harness; `harness:adi`
    /// picks its model provider via the `provider` extra).
    pub backend: String,
    /// The system prompt seeding the agent (the resolved prompt body). May be empty.
    #[serde(default)]
    pub system_prompt: String,
    /// The CLI command scope this agent may use, e.g. `tasks,projects`. This is stored in the
    /// historical `tools` field for compatibility with existing manifests.
    #[serde(default)]
    pub tools: String,
    /// Backend-specific model alias, e.g. `opus`/`sonnet` (claude), `gpt-5-codex` (codex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Claude-engine permission mode, e.g. `default` | `acceptEdits` | `plan` |
    /// `bypassPermissions` (applies to `*:claude` and `harness:claude-sdk`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// `harness:adi` sampling temperature (only meaningful for providers that accept it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Optional cap on the number of agent turns per run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// Free-form tags. A tag equal to an agent name is what auto-assigns/auto-starts a task
    /// (docs/adi-agents.md §9) — the dispatch hook, once orchestration exists.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Pinned in the UI / preferred for quick-dispatch.
    #[serde(default)]
    pub starred: bool,
    /// The project this agent is filed under (its [`adi-projects`] id), or `None` for a
    /// global agent. Pure metadata: it scopes where the agent shows up (a project's detail
    /// page), not what it may do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Backend-specific fields not yet promoted to first-class manifest properties.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
    /// When the definition was created, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
    /// When the definition was last saved, as Unix epoch seconds.
    #[serde(default)]
    pub updated_at: u64,
}

/// A registered agent: its name (the file stem under `agents/`) plus its loaded
/// [`AgentManifest`]. The name is not stored in the file — it *is* the file. `Serialize` so the
/// CLI/API can emit it; built from disk, never deserialized, so no `Deserialize`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agent {
    /// The agent name — its `<name>.toml` file stem under `~/.adi/mono/agents/`.
    pub name: String,
    /// The parsed manifest.
    pub manifest: AgentManifest,
}

impl AgentManifest {
    /// The executor (`tmux` / `process` / `harness`) — the part before the `:` in
    /// [`Self::backend`]; empty string if the backend has no `executor:` prefix. Drives how the
    /// agent runs and which params apply.
    #[must_use]
    pub fn executor(&self) -> &str {
        self.backend.split_once(':').map_or("", |(executor, _)| executor)
    }
}

/// The current time as Unix epoch seconds (0 if the clock predates the epoch).
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

    #[test]
    fn executor_is_the_prefix_before_the_colon() {
        let mut m = AgentManifest::default();
        m.backend = "tmux:claude".into();
        assert_eq!(m.executor(), "tmux");
        m.backend = "process:codex".into();
        assert_eq!(m.executor(), "process");
        m.backend = "harness:claude-sdk".into();
        assert_eq!(m.executor(), "harness");
        m.backend = "weird".into();
        assert_eq!(m.executor(), "");
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
