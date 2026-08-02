//! What a runner reports while it works — normalized, so a reader never learns an engine's wire
//! format.
//!
//! Each engine says it differently (Claude's `stream-json`, Codex's `--json`, the ADI loop's own
//! events), and translating is the runner's job precisely because it is the only layer that should
//! know. What crosses the boundary is [`RunEvent`], built from the same [`Step`] / [`TurnMetrics`]
//! types the rest of the platform already renders.

use crate::progress::{Step, TurnMetrics};

/// One thing that happened during a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    /// An item on the turn's timeline: something said mid-turn, a reasoning block, or a tool call
    /// and its result.
    Step(Step),
    /// The turn's final message — its answer.
    Answer { text: String },
    /// Per-turn telemetry from the engine's closing event.
    Metrics(TurnMetrics),
    /// The turn is over. `error` carries the reason when it ended badly.
    Finished { ok: bool, error: Option<String> },
}

/// Which [`RunEvent`] kinds a runner can *ever* produce.
///
/// A descriptor, not a promise about any particular turn: a runner that can emit tool steps still
/// emits none for a turn that used no tools. Readers use this to decide what to render at all —
/// which cannot be answered by attempting to read and catching the failure.
///
/// A terminal runner declares all of these `false`: there is nothing structured to read off a tty,
/// and its screen is reached through [`Terminal::capture`](super::Terminal::capture) instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventKinds {
    pub message: bool,
    pub tool_call: bool,
    pub thinking: bool,
    pub metrics: bool,
}

impl EventKinds {
    /// Whether this runner reports anything at all through the event stream.
    #[must_use]
    pub fn any(self) -> bool {
        self.message || self.tool_call || self.thinking || self.metrics
    }
}

/// A read of the event stream: what happened after the cursor asked for, and the cursor to ask with
/// next time.
///
/// The cursor is minted by the runner and opaque to everyone else — a byte offset into a log for a
/// local runner, an event id for one reading somebody else's API. Persisting it is the caller's
/// business; understanding it is not.
#[derive(Debug, Clone, PartialEq)]
pub struct EventBatch {
    pub events: Vec<RunEvent>,
    pub cursor: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_declares_nothing_and_says_so() {
        assert!(!EventKinds::default().any());
        assert!(
            EventKinds {
                metrics: true,
                ..EventKinds::default()
            }
            .any()
        );
    }
}
