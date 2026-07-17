//! The backend identifier on a manifest: `<executor>:<engine>`. Built-in backends this crate
//! can run and validate are named variants; any other (plugin, harness, or empty) is kept
//! verbatim in [`Backend::Other`] so it round-trips through the TOML store unchanged.

use std::fmt;

use serde::{Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    TmuxClaude,
    TmuxCodex,
    ProcessClaude,
    ProcessCodex,
    Wasm,
    /// A backend this crate doesn't run itself — a plugin/harness backend, or the empty default.
    Other(String),
}

impl Backend {
    /// The wire string for this backend (`process:claude`, `harness:adi`, `""`, …).
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::TmuxClaude => "tmux:claude",
            Self::TmuxCodex => "tmux:codex",
            Self::ProcessClaude => "process:claude",
            Self::ProcessCodex => "process:codex",
            Self::Wasm => "wasm:loop-script",
            Self::Other(value) => value,
        }
    }

    /// The executor (`tmux` / `process` / `wasm` / `harness`) — the part before the `:`. An
    /// [`Other`](Self::Other) backend with no `:` (or the empty default) has no executor: `""`.
    pub(crate) fn executor(&self) -> &str {
        match self {
            Self::TmuxClaude | Self::TmuxCodex => "tmux",
            Self::ProcessClaude | Self::ProcessCodex => "process",
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
            "tmux:claude" => Self::TmuxClaude,
            "tmux:codex" => Self::TmuxCodex,
            "process:claude" => Self::ProcessClaude,
            "process:codex" => Self::ProcessCodex,
            "wasm:loop-script" => Self::Wasm,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<String> for Backend {
    fn from(value: String) -> Self {
        match value.as_str() {
            "tmux:claude" => Self::TmuxClaude,
            "tmux:codex" => Self::TmuxCodex,
            "process:claude" => Self::ProcessClaude,
            "process:codex" => Self::ProcessCodex,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_backends_round_trip_through_strings() {
        for wire in [
            "tmux:claude",
            "tmux:codex",
            "process:claude",
            "process:codex",
            "wasm:loop-script",
        ] {
            let backend = Backend::from(wire);
            assert!(!matches!(backend, Backend::Other(_)), "{wire} should be named");
            assert_eq!(backend.as_str(), wire);
            assert_eq!(Backend::from(wire.to_string()), backend);
        }
    }

    #[test]
    fn unknown_and_empty_backends_are_kept_verbatim() {
        for wire in ["harness:adi", "cloud:worker", "tmux:unknown", "weird", ""] {
            let backend = Backend::from(wire);
            assert_eq!(backend, Backend::Other(wire.to_string()));
            assert_eq!(backend.as_str(), wire);
        }
    }

    #[test]
    fn executor_is_the_prefix_before_the_colon() {
        assert_eq!(Backend::TmuxClaude.executor(), "tmux");
        assert_eq!(Backend::ProcessCodex.executor(), "process");
        assert_eq!(Backend::Wasm.executor(), "wasm");
        assert_eq!(Backend::from("harness:adi").executor(), "harness");
        assert_eq!(Backend::from("weird").executor(), "");
        assert_eq!(Backend::default().executor(), "");
    }
}
