//! The backend identifier on a manifest: `<executor>:<engine>`. Built-in backends this crate
//! can run and validate are named variants; any other (plugin, harness, or empty) is kept
//! verbatim in [`Backend::Other`] so it round-trips through the TOML store unchanged.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    PtyClaude,
    PtyCodex,
    ProcessClaude,
    ProcessCodex,
    /// The `claude` CLI driven headless by ADI's harness (a turn-capped, adi-scoped print run).
    HarnessClaudeSdk,
    /// ADI's own answering loop; the model provider is the manifest's `provider` argument. Runnable
    /// once a supported provider (Anthropic, or a local Ollama) is configured — each turn calls that
    /// provider's chat API over the conversation transcript.
    HarnessAdi,
    Wasm,
    /// A backend this crate doesn't run itself — a plugin backend, or the empty default.
    Other(String),
}

impl Backend {
    /// The wire string for this backend (`process:claude`, `harness:adi`, `""`, …).
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::PtyClaude => "pty:claude",
            Self::PtyCodex => "pty:codex",
            Self::ProcessClaude => "process:claude",
            Self::ProcessCodex => "process:codex",
            Self::HarnessClaudeSdk => "harness:claude-sdk",
            Self::HarnessAdi => "harness:adi",
            Self::Wasm => "wasm:loop-script",
            Self::Other(value) => value,
        }
    }

    /// The executor (`pty` / `process` / `harness` / `wasm`) — the part before the `:`. An
    /// [`Other`](Self::Other) backend with no `:` (or the empty default) has no executor: `""`.
    pub(crate) fn executor(&self) -> &str {
        match self {
            Self::PtyClaude | Self::PtyCodex => "pty",
            Self::ProcessClaude | Self::ProcessCodex => "process",
            Self::HarnessClaudeSdk | Self::HarnessAdi => "harness",
            Self::Wasm => "wasm",
            Self::Other(value) => value.split_once(':').map_or("", |(executor, _)| executor),
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::Other(String::new())
    }
}

impl From<&str> for Backend {
    fn from(value: &str) -> Self {
        match value {
            "pty:claude" => Self::PtyClaude,
            "pty:codex" => Self::PtyCodex,
            // Back-compat: manifests written before the tmux→pty rename still carry the old wire
            // strings; map them onto the pty backends so stored agents keep running.
            "tmux:claude" => Self::PtyClaude,
            "tmux:codex" => Self::PtyCodex,
            "process:claude" => Self::ProcessClaude,
            "process:codex" => Self::ProcessCodex,
            "harness:claude-sdk" => Self::HarnessClaudeSdk,
            "harness:adi" => Self::HarnessAdi,
            "wasm:loop-script" => Self::Wasm,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<String> for Backend {
    fn from(value: String) -> Self {
        match value.as_str() {
            "pty:claude" => Self::PtyClaude,
            "pty:codex" => Self::PtyCodex,
            // Back-compat with the legacy tmux wire strings (see the `&str` impl above).
            "tmux:claude" => Self::PtyClaude,
            "tmux:codex" => Self::PtyCodex,
            "process:claude" => Self::ProcessClaude,
            "process:codex" => Self::ProcessCodex,
            "harness:claude-sdk" => Self::HarnessClaudeSdk,
            "harness:adi" => Self::HarnessAdi,
            "wasm:loop-script" => Self::Wasm,
            // Reuse the already-owned string instead of re-allocating.
            _ => Self::Other(value),
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Serialize as the bare wire string, so the stored manifest keeps `backend = "process:claude"`.
impl Serialize for Backend {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Deserialize from the bare wire string, mapping known values onto named variants.
impl<'de> Deserialize<'de> for Backend {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_backends_round_trip_through_strings() {
        for wire in [
            "pty:claude",
            "pty:codex",
            "process:claude",
            "process:codex",
            "harness:claude-sdk",
            "harness:adi",
            "wasm:loop-script",
        ] {
            let backend = Backend::from(wire);
            assert!(
                !matches!(backend, Backend::Other(_)),
                "{wire} should be named"
            );
            assert_eq!(backend.as_str(), wire);
            assert_eq!(Backend::from(wire.to_string()), backend);
        }
    }

    #[test]
    fn legacy_tmux_wire_strings_map_to_pty() {
        // Manifests stored before the tmux→pty rename must keep working: the legacy strings decode
        // onto the pty backends (and re-serialize as the new `pty:*` names), while an unknown
        // `tmux:*` engine is still kept verbatim as `Other`.
        assert_eq!(Backend::from("tmux:claude"), Backend::PtyClaude);
        assert_eq!(Backend::from("tmux:claude").as_str(), "pty:claude");
        assert_eq!(Backend::from("tmux:codex"), Backend::PtyCodex);
        assert_eq!(Backend::from("tmux:codex").as_str(), "pty:codex");
        assert_eq!(Backend::from("tmux:claude".to_string()), Backend::PtyClaude);
        assert_eq!(
            Backend::from("tmux:unknown"),
            Backend::Other("tmux:unknown".to_string())
        );
    }

    #[test]
    fn unknown_and_empty_backends_are_kept_verbatim() {
        for wire in [
            "cloud:worker",
            "harness:unknown",
            "pty:unknown",
            "weird",
            "",
        ] {
            let backend = Backend::from(wire);
            assert_eq!(backend, Backend::Other(wire.to_string()));
            assert_eq!(backend.as_str(), wire);
        }
    }

    #[test]
    fn executor_is_the_prefix_before_the_colon() {
        assert_eq!(Backend::PtyClaude.executor(), "pty");
        assert_eq!(Backend::ProcessCodex.executor(), "process");
        assert_eq!(Backend::HarnessClaudeSdk.executor(), "harness");
        assert_eq!(Backend::HarnessAdi.executor(), "harness");
        assert_eq!(Backend::Wasm.executor(), "wasm");
        assert_eq!(Backend::from("harness:plugin").executor(), "harness");
        assert_eq!(Backend::from("weird").executor(), "");
        assert_eq!(Backend::default().executor(), "");
    }
}
