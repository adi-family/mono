//! What actually starts, watches, and stops an agent's work — and nothing else.
//!
//! A runner is a **process manager**, not a place to keep information. Everything durable about a
//! session (what it is called, when it started, whether a reader hid it, what is queued behind it,
//! what it said) belongs to the session store; a runner only starts things, says whether they are
//! alive, stops them, and translates its own engine's output into normalized events.
//!
//! That split is why this trait carries no `run_id`, no `sessions_dir`, no `hidden`, and no queue.
//! It is also why there is no `Handle` in any signature: a runner that needs a local token keeps it
//! in its own [state slot](Session::state), and a runner that needs none — one whose liveness is a
//! request to somebody else's API — simply writes nothing there.
//!
//! See `docs/agent-runner.md` for the full design.

pub mod detached;
mod event;
pub mod pty;
mod registry;
mod session;
mod spec;

use std::time::Duration;

use crate::error::Result;

pub use event::{EventBatch, EventKinds, RunEvent};
pub use registry::runner_for;
pub use session::{Session, StateWriter};
pub use spec::RunSpec;

/// Which runner this is — for labels, telemetry, and the session record.
///
/// Open rather than an enum, on the same reasoning as [`Backend::Other`](crate::Backend): a runner
/// that ships outside this crate still needs a name, and adding one must not mean editing a type
/// here.
///
/// **Never match on this to reach behaviour.** `pty`, a future `tmux-pty`, and a `cloud-pty` are
/// three kinds and one behaviour; a call site that switches on the name grows an arm per runner and
/// still has no way to *call* anything. Capability extensions ([`Runner::as_terminal`]) are how you
/// ask instead, and they answer for all three at once.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunnerKind(pub String);

impl RunnerKind {
    #[must_use]
    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunnerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a [`Runner::stop`] found and did. `forced` says the grace period ran out and the runner had
/// to be rude about it — worth recording, since a run that never exits cooperatively is a fact about
/// the engine rather than about the click that stopped it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stopped {
    pub was_running: bool,
    pub forced: bool,
}

pub trait Runner: Send + Sync {
    fn kind(&self) -> RunnerKind;

    /// Whether this spec could run at all: its arguments parse, its engine is configured. No side
    /// effects — this is the fail-fast gate asked before anything touches disk, and the honest
    /// answer to "would the Run button work".
    ///
    /// # Errors
    /// Returns the reason it could not run, ready to show.
    fn check(&self, spec: &RunSpec) -> Result<()>;

    /// The only start verb. A session that already exists is **resumed**; one that does not is
    /// established. The caller never chooses between the two — that combinatorial pair is exactly
    /// what this interface exists to not have. The runner decides from
    /// [`Session::has_started`] and whatever it kept in its state slot.
    ///
    /// # Errors
    /// Returns argument, configuration, or spawn errors.
    fn send(&self, spec: &RunSpec, session: &dyn Session, message: &str) -> Result<()>;

    /// Whether this session's work is still in flight.
    ///
    /// Cheap for a local child (a signal-0 probe); a network round trip for a runner whose engine
    /// lives elsewhere. Callers on a poll path should cache rather than ask on every tick.
    fn is_alive(&self, session: &dyn Session) -> bool;

    /// Stop the session's work, escalating to a forced kill once `grace` has elapsed.
    /// [`Duration::ZERO`] forces immediately.
    ///
    /// The escalation lives here, inside the runner, because it is engine-specific: TERM-then-KILL
    /// for a local child, cancel-then-force-cancel for an API, and nothing at all for a runner with
    /// no rude mode — which waits out the grace and honestly reports `forced: false` while still
    /// running.
    ///
    /// # Errors
    /// Returns signalling or transport errors.
    fn stop(&self, session: &dyn Session, grace: Duration) -> Result<Stopped>;

    /// Which event kinds this runner can *ever* emit. The capability descriptor a reader renders
    /// from — you cannot decide whether to draw a thinking pane by trying to read one and catching
    /// the failure.
    fn emits(&self) -> EventKinds;

    /// Whether a later [`send`](Runner::send) continues the same thread rather than starting an
    /// unrelated one.
    ///
    /// Data, not an attempt: a reader has to decide whether to draw a reply box *before* anybody
    /// types in it. `false` is the honest default — an engine invoked one-shot really does forget
    /// everything between calls, and offering to continue a conversation it cannot continue is worse
    /// than not offering.
    fn resumes(&self) -> bool {
        false
    }

    /// This session's normalized events after `cursor`, plus the cursor to resume from.
    ///
    /// Pull rather than push: a pushed sink only works inside the process that spawned the run, and
    /// runs started by the CLI or a trigger are read in the app. Pulling with an opaque cursor works
    /// from any process and survives a restart.
    ///
    /// # Errors
    /// Returns read or transport errors. An unparseable log is not an error — it yields whatever
    /// text could be recovered.
    fn events(&self, session: &dyn Session, cursor: Option<&serde_json::Value>)
    -> Result<EventBatch>;

    /// The terminal extension, for runners that drive a live screen. Everyone else inherits `None`.
    ///
    /// This is the door capability-specific behaviour goes through, so a caller asks one question
    /// ("are you a terminal?") instead of enumerating kinds.
    fn as_terminal(&self) -> Option<&dyn Terminal> {
        None
    }
}

/// Driving a live screen: the things that only make sense when there is a terminal to type into.
///
/// Deliberately not on [`Runner`]. A pty cannot produce message events (there is nothing structured
/// to read off a tty) and its screen is a mutable snapshot rather than an append stream, so it
/// declares an empty [`Runner::emits`] and is read through [`capture`](Terminal::capture) instead —
/// which keeps the whole shape of "terminal" in one place rather than leaking into the event model.
pub trait Terminal {
    /// Type into the live session.
    ///
    /// # Errors
    /// Returns validation, missing-session, or pty errors.
    fn send_keys(&self, session: &dyn Session, text: &str, key: &str) -> Result<()>;

    /// The current screen, or `None` when there is nothing to capture.
    fn capture(&self, session: &dyn Session) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_just_its_name() {
        let kind = RunnerKind::new("cloud-pty");
        assert_eq!(kind.as_str(), "cloud-pty");
        assert_eq!(kind.to_string(), "cloud-pty");
    }

    /// Stopping something that was already gone is not a force — the two flags answer different
    /// questions, and a caller reading `forced` needs `was_running` to make sense of it.
    #[test]
    fn a_default_stop_found_nothing_and_forced_nothing() {
        let stopped = Stopped::default();
        assert!(!stopped.was_running);
        assert!(!stopped.forced);
    }
}
