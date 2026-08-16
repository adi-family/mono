//! Cross-backend "progress of answering": the structured activity of one turn/run — its tool calls
//! and thinking (`Step`), its telemetry (`TurnMetrics`) — plus a per-backend capability descriptor
//! ([`BackendCapabilities`]) that says which of these a backend can actually surface.
//!
//! Each engine emits progress its own way (Claude's `stream-json`, Codex's `--json`, the ADI loop's
//! own events), and each *runner* parses the formats that are its own into this common shape. There
//! is deliberately no `parse(backend, log)` here any more: a table of every engine's wire format,
//! sitting a layer above the code that knows them, was one more place that had to be taught about a
//! new backend — and one more place to forget. A backend with no structured output (or an old
//! plain-text log) simply yields text-only content, via [`text_of`].

use serde::{Deserialize, Serialize};

use crate::backend::Backend;

/// How much of a turn's log is parsed for progress — generous, since a tool-using turn's event
/// stream is larger than a plain answer but still bounded.
pub(crate) const MAX_PARSE_BYTES: u64 = 2 * 1024 * 1024;

/// One item on a turn's timeline, in the order the engine emitted it.
///
/// An agent's turn is not "one answer plus a pile of tool calls" — it is a *sequence*: it says
/// something, runs tools, says something else, runs more tools, and finally answers. [`Step`] keeps
/// that sequence intact, so a reader can follow what happened rather than reading a single blob of
/// glued-together commentary. The turn's **final** message lands in [`TurnContent::text`]; every
/// message it wrote *before* that one stays here as a [`Step::Message`], in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    /// Something the agent said mid-turn, between tool calls — its running commentary. The last
    /// such message is the turn's answer and lives in [`TurnContent::text`] instead, never here.
    Message { text: String },
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

/// A tool step's lifecycle: still running, finished ok, failed — or never answered at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
    /// The call went out and nothing came back: the run ended — stopped, killed, out of budget —
    /// while that call was still in flight. Deliberately not [`Error`](Self::Error), which is a
    /// tool that *answered* and said no; here the tool said nothing at all, and blaming it for a
    /// run somebody interrupted would put it in the failed column of every tally.
    Unanswered,
}

/// Close a finished turn's timeline: a call still marked [`ToolStatus::Running`] once the run is
/// over is a claim about a process that no longer exists.
///
/// The last thing an interrupted turn did is almost always a call whose result never landed — the
/// engine was killed between writing the invocation and reading the answer — so without this every
/// interrupted conversation keeps a live-looking call at the top of it for ever. Called wherever a
/// turn is known to be over: on the way into the transcript, on the way back out of it (rows
/// recorded before this existed), and on the live parse of a run whose child has exited.
pub(crate) fn close_open_calls(steps: &mut [Step]) {
    for step in steps {
        if let Step::Tool { status, .. } = step
            && *status == ToolStatus::Running
        {
            *status = ToolStatus::Unanswered;
        }
    }
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
    /// How the engine says the turn ended, in its own vocabulary — `completed`, `api_error`,
    /// `aborted_tools`, and so on. Carried rather than folded into [`is_error`](Self::is_error)
    /// because the distinctions are what a reader actually acts on: a run that ran out of
    /// authorization and one whose tools were cut off are both errors and want opposite responses.
    /// `None` from an engine that reports no such thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
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
    /// A message to it may carry images — what decides whether a composer offers to attach one.
    #[serde(default)]
    pub images: bool,
}

/// What a backend can surface, **asked of the runner that runs it** rather than kept as a matrix
/// here.
///
/// A hand-maintained table is a second truth: it says a backend reports thinking while the runner
/// that parses its stream knows it does not, and nothing makes the two agree. So each flag is
/// derived from what the runner already has to answer anyway — whether it drives a terminal, whether
/// a later send continues the same thread, and which event kinds it can ever emit.
///
/// A backend nothing here runs keeps the all-false descriptor.
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
        images: false,
    };
    let Some(runner) = crate::runner::runner_for(backend) else {
        return base;
    };
    let interactive = runner.as_terminal().is_some();
    let kinds = runner.emits();
    BackendCapabilities {
        interactive,
        history: !interactive,
        // Continuing a thread you *type* into is not a reply box, so a terminal never claims it
        // however resumable its session is.
        answerable: runner.resumes() && !interactive,
        // Anything with a runner produces something to watch — a live pane, or a log to tail.
        live_text: true,
        tool_steps: kinds.tool_call,
        thinking: kinds.thinking,
        metrics: kinds.metrics,
        images: runner.takes_images(),
    }
}

/// A best-effort UTF-8 view of a log, trimmed — the answer for a backend with no structured stream,
/// and for a log written by something that died before it could say anything.
pub(crate) fn text_of(log: &[u8]) -> String {
    String::from_utf8_lossy(log).trim().to_string()
}

/// Taking `&bool` is what serde's `skip_serializing_if` requires — it hands the predicate a
/// reference to the field, so the by-value form clippy asks for cannot be named there.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The matrix, spelled out. [`capabilities`] is now *derived* from each backend's runner, and
    /// this is the check that deriving it did not quietly change what a reader renders: every flag
    /// below is the value the hand-maintained table returned before the runners existed.
    ///
    /// Read as (interactive, history, answerable, live_text, tool_steps, thinking, metrics, images).
    ///
    /// `images` is the newest column, and true for every engine that can be shown a picture *by any
    /// route* — the adi loop puts the bytes in the request body it writes, and the three CLI engines
    /// are told where the file is and open it themselves. Only a live terminal cannot: its launch
    /// message is never typed at all.
    #[test]
    fn the_derived_matrix_is_the_one_the_ui_has_always_rendered() {
        for (wire, want) in [
            (
                "pty:claude",
                [true, false, false, true, false, false, false, false],
            ),
            (
                "pty:codex",
                [true, false, false, true, false, false, false, false],
            ),
            (
                "process:claude",
                [false, true, false, true, true, true, true, true],
            ),
            (
                "process:codex",
                [false, true, false, true, true, false, true, true],
            ),
            (
                "harness:claude-sdk",
                [false, true, true, true, true, true, true, true],
            ),
            (
                "harness:adi",
                [false, true, true, true, true, false, true, true],
            ),
            (
                "cloud:worker",
                [false, false, false, false, false, false, false, false],
            ),
        ] {
            let got = capabilities(&Backend::from(wire));
            assert_eq!(
                [
                    got.interactive,
                    got.history,
                    got.answerable,
                    got.live_text,
                    got.tool_steps,
                    got.thinking,
                    got.metrics,
                    got.images,
                ],
                want,
                "{wire} renders differently than it used to",
            );
        }
    }
}
