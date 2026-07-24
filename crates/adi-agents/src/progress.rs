//! Cross-backend "progress of answering": the structured activity of one turn/run — its tool calls
//! and thinking (`Step`), its telemetry (`TurnMetrics`) — plus a per-backend capability descriptor
//! ([`BackendCapabilities`]) that says which of these a backend can actually surface.
//!
//! Each engine emits progress its own way (Claude's `stream-json`, Codex's `--json`, the ADI loop's
//! own events); [`parse`] turns a run/turn's captured log into a common [`TurnContent`] regardless.
//! A backend with no structured output (or an old plain-text log) simply yields text-only content.

use serde::{Deserialize, Serialize};

use crate::backend::Backend;

/// How much of a turn's log is parsed for progress — generous, since a tool-using turn's event
/// stream is larger than a plain answer but still bounded.
pub(crate) const MAX_PARSE_BYTES: u64 = 2 * 1024 * 1024;

/// One activity within a turn. The answer *text* is not a step — it lands in [`TurnContent::text`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    /// A model reasoning block (shown dim/collapsed).
    Thinking { text: String },
    /// A tool invocation and, once it returns, its result.
    Tool {
        name: String,
        /// The rendered arguments (compact JSON or short text).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        input: String,
        status: ToolStatus,
        /// The tool's output/result once it returns (empty while running).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        output: String,
    },
}

/// A tool step's lifecycle: still running, finished ok, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
}

/// Per-turn telemetry from the engine's final event. Cost is kept in micro-dollars (integer) so the
/// whole model stays `Eq` — the poll-change comparisons and jsonl round-trips depend on it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMetrics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Cost in micro-dollars (1e-6 USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Model round-trips the engine reported for the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
    /// Tools blocked by permission, if the engine reports any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_denials: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

impl TurnMetrics {
    /// Whether any field carries information — used to drop empty metrics rather than persist `{}`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == TurnMetrics::default()
    }
}

/// A turn/run's parsed content: the answer text, its activity steps, and its metrics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnContent {
    pub text: String,
    pub steps: Vec<Step>,
    pub metrics: Option<TurnMetrics>,
}

/// What a backend can surface — the single source of truth the API reports and the UI renders from,
/// consolidating the old ad-hoc interactive/answerable flags with the new progress features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCapabilities {
    /// A live pane you type into (pty): the session is driven with keystrokes, not turn replies.
    pub interactive: bool,
    /// Runs/conversations persist as a browsable history (false for the single ephemeral pty pane).
    pub history: bool,
    /// You can reply into a turn to continue the same thread (conversations only).
    pub answerable: bool,
    /// Produces streaming text output (a pane, or a log tail).
    pub live_text: bool,
    /// Surfaces structured tool-call steps.
    pub tool_steps: bool,
    /// Surfaces model thinking/reasoning steps.
    pub thinking: bool,
    /// Reports per-turn metrics (tokens / cost / duration).
    pub metrics: bool,
}

/// The capability profile for a backend. This is the honest matrix: pty is a live pane with no
/// history/replies/steps; process runs keep history and (for the CLIs that emit events) steps, but
/// are one-shot; harness runs additionally answer; wasm contributes only dispatch metrics.
#[must_use]
pub fn capabilities(backend: &Backend) -> BackendCapabilities {
    let base = BackendCapabilities {
        interactive: false,
        history: false,
        answerable: false,
        live_text: false,
        tool_steps: false,
        thinking: false,
        metrics: false,
    };
    match backend {
        Backend::PtyClaude | Backend::PtyCodex => BackendCapabilities {
            interactive: true,
            live_text: true,
            ..base
        },
        Backend::ProcessClaude => BackendCapabilities {
            history: true,
            live_text: true,
            tool_steps: true,
            thinking: true,
            metrics: true,
            ..base
        },
        Backend::ProcessCodex => BackendCapabilities {
            history: true,
            live_text: true,
            tool_steps: true,
            metrics: true,
            ..base
        },
        Backend::HarnessClaudeSdk => BackendCapabilities {
            history: true,
            answerable: true,
            live_text: true,
            tool_steps: true,
            thinking: true,
            metrics: true,
            ..base
        },
        Backend::HarnessAdi => BackendCapabilities {
            history: true,
            answerable: true,
            live_text: true,
            tool_steps: true,
            metrics: true,
            ..base
        },
        Backend::Wasm => BackendCapabilities {
            // Dispatched synchronously (no run panel), but its outcome carries turns/tokens.
            metrics: true,
            ..base
        },
        Backend::Other(_) => base,
    }
}

/// Parse a run/turn's captured log into its [`TurnContent`], per the backend's engine format. An
/// unrecognised or plain-text log (old logs, non-streaming output) yields text-only content, so this
/// never fails — the worst case is "just the answer, no steps".
#[must_use]
pub fn parse(backend: &Backend, log: &[u8]) -> TurnContent {
    match backend {
        Backend::ProcessClaude | Backend::HarnessClaudeSdk => {
            crate::backends::claude_stream::parse(log)
        }
        // Codex `--json` and the ADI loop's native events are wired later; until then their logs
        // parse as plain text.
        _ => TurnContent {
            text: text_of(log),
            steps: Vec::new(),
            metrics: None,
        },
    }
}

/// A best-effort UTF-8 view of a log, trimmed — the plain-text fallback answer.
pub(crate) fn text_of(log: &[u8]) -> String {
    String::from_utf8_lossy(log).trim().to_string()
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}
