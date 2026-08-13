//! [`HumanRunner`] — the runner for a run whose model is a person.
//!
//! Everything else in this tree manages a process: it builds an argv, spawns a child, watches for
//! its exit, and reads the stream it prints. This one manages a browser. There is nothing to spawn,
//! nothing to signal, and no exit to wait for — the "engine" is somebody reading the prompt and
//! deciding what the model would emit.
//!
//! # What it still is
//!
//! A real run, and that is the whole point of it being a runner rather than a mode. A simulated run
//! opens a session in the same store, keeps a transcript in the same table, writes the same event
//! log in the same format, is listed by `agents runs`, records an outcome, and publishes
//! `adi.agents.run.finished`. None of that is written twice: it falls out of the session being an
//! ordinary session whose runner happens to have no child.
//!
//! It also runs the agent's **own** environment. The spec it is handed came off the same
//! [`launch_spec`](crate::Agents) path a real launch uses, so the cwd, the `.bin` shims, the `PATH`,
//! the secrets and the composed prompt are the run's, not a re-derivation of them. That is what
//! makes the tools the person calls really be the agent's tools.
//!
//! # Where the turn actually comes from
//!
//! Not from here. A detached runner's child writes the turn's events into the log and this runner
//! would read them back — but there is no child, so the events are written by the agent layer as
//! the person emits them, into the very same log in the very same format
//! ([`adi_events`](crate::backends::adi_events)). This runner then reads them back exactly as
//! [`DetachedRunner`](super::detached::DetachedRunner) reads the adi loop's.
//!
//! That is the same exception `docs/agent-runner.md` already carves out for the adi loop: every
//! other engine answers a turn in a child the runner spawned and watches, while the adi loop *is*
//! that child. Here the browser is.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::backends::adi_events;
use crate::error::Result;
use crate::progress::MAX_PARSE_BYTES;

use super::{
    EventBatch, EventKinds, RunEvent, RunSpec, Runner, RunnerKind, Session, Stopped,
};

/// The name this runner is recorded under, and the one a session record is resolved back through.
pub const KIND: &str = "human";

/// The state slot: whether a person is currently in the seat.
///
/// One flag, because that is the only thing this runner knows that the store does not. There is no
/// pid to keep and no engine session to resume — the transcript *is* the thread, and it belongs to
/// the store.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct State {
    /// Whether the turn is open — the person has been handed the prompt and has not ended it yet.
    #[serde(default)]
    open: bool,
    /// The system prompt this run was composed with, frozen at its first turn.
    ///
    /// Kept rather than re-derived on every read, for the reason
    /// [`RunSpec::tool_help`](super::RunSpec::tool_help) is kept: composing one means assembling a
    /// spec, and assembling a spec syncs the agent's `.bin` and asks every tool to describe itself.
    /// That is a write path, and this is read by a browser polling a page. Freezing it also makes
    /// it *true* — what the person is reading is what the run opened with, not what it would open
    /// with if it were launched again now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
}

impl State {
    fn of(session: &dyn Session) -> Self {
        session
            .state()
            .and_then(|s| serde_json::from_value(s).ok())
            .unwrap_or_default()
    }
}

/// A run driven by a person taking the model's seat.
#[derive(Debug, Clone, Copy, Default)]
pub struct HumanRunner;

impl HumanRunner {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Mark the seat occupied, or vacated, leaving everything else in the slot alone.
    fn set_open(session: &dyn Session, open: bool) -> Result<()> {
        let mut state = State::of(session);
        state.open = open;
        session.set_state(json!(state))
    }

    /// The system prompt this run was composed with — what the person in the seat is shown.
    ///
    /// `None` before the run's first turn, which is the only moment it does not have one.
    #[must_use]
    pub fn prompt_of(session: &dyn Session) -> Option<String> {
        State::of(session).prompt
    }
}

impl Runner for HumanRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::new(KIND)
    }

    /// Always runnable.
    ///
    /// Not a stub: every other runner's `check` asks whether its engine is configured and its
    /// arguments parse, and here there is no engine to configure. A simulated run of an agent
    /// pointed at a provider nobody has a key for is *exactly* the run somebody wants — reading the
    /// prompt is the point, and refusing it because the model behind it is unreachable would refuse
    /// the one case the feature exists for.
    fn check(&self, _spec: &RunSpec) -> Result<()> {
        Ok(())
    }

    /// Take the turn. Nothing is spawned; the seat is marked occupied and the log is opened fresh.
    ///
    /// The message itself is already in the transcript — the agent layer appends it before calling,
    /// as it does for every runner — so there is nothing to carry anywhere. Truncating the log is
    /// what a harness turn does too: the log slot is per-turn, and a cursor left past the end of it
    /// by the previous turn is handled on read.
    fn send(&self, spec: &RunSpec, session: &dyn Session, _message: &str) -> Result<()> {
        let mut state = State::of(session);
        // Composed on the first turn and kept. Through the one composer every runner here uses, so
        // what the person reads is the string a real run of this agent is handed — which is the
        // whole claim the feature makes.
        if state.prompt.is_none() {
            state.prompt = super::prompt::compose(spec, None);
        }
        state.open = true;
        // Created, not merely emptied: the log existing is what says a turn has started, so this is
        // also what makes the next `send` a resume rather than an establish.
        std::fs::write(session.log_path(), b"")?;
        session.set_state(json!(state))
    }

    /// Whether somebody is still in the seat.
    ///
    /// Truthfully a slow answer — a person can be at lunch — and deliberately not timed out here. A
    /// simulated turn is open until it is ended or stopped, and a runner that decided on its own
    /// that a reader had been gone too long would close a turn somebody was still writing.
    fn is_alive(&self, session: &dyn Session) -> bool {
        State::of(session).open
    }

    /// Close the turn. There is nothing to signal, so there is nothing to escalate to and `forced`
    /// is never true — the grace period is spent by nobody.
    fn stop(&self, session: &dyn Session, _grace: Duration) -> Result<Stopped> {
        let was_running = State::of(session).open;
        if was_running {
            Self::set_open(session, false)?;
        }
        Ok(Stopped { was_running, forced: false })
    }

    fn emits(&self) -> EventKinds {
        EventKinds {
            message: true,
            tool_call: true,
            // A person does not think in tokens the way a model streams them, and there is no
            // separate channel for it: what would be thinking is just something said.
            thinking: false,
            // Reported, and worth reporting: the prompt has a real token count and the turn has a
            // real count of rounds. Cost is the honest zero.
            metrics: true,
        }
    }

    /// A simulated run is a conversation, and answering it again continues the same one.
    fn resumes(&self) -> bool {
        true
    }

    /// The turn's events, read out of the log the agent layer wrote them into.
    ///
    /// Same format and same parser as the adi loop's, so a simulated turn's timeline is folded,
    /// settled and displayed by code that has no idea it was typed by a person.
    fn events(&self, session: &dyn Session, cursor: Option<&Value>) -> Result<EventBatch> {
        let offset = cursor.and_then(Value::as_u64).unwrap_or(0);
        let (bytes, next) = read_after(session.log_path(), offset);
        let content = adi_events::parse(&bytes);

        let mut events: Vec<RunEvent> = content.steps.into_iter().map(RunEvent::Step).collect();
        if !content.text.trim().is_empty() {
            events.push(RunEvent::Answer { text: content.text });
        }
        if let Some(metrics) = content.metrics {
            events.push(RunEvent::Metrics(metrics));
        }
        Ok(EventBatch { events, cursor: json!(next) })
    }
}

/// The log from `offset`, capped, with the offset to resume from.
///
/// A log that isn't there is not an error — a session created and not yet sent to has none, which
/// is the normal first poll. An offset past the end is a cursor from a previous turn that has since
/// been truncated: start over rather than reading nothing for ever.
fn read_after(path: &Path, offset: u64) -> (Vec<u8>, u64) {
    let Ok(bytes) = std::fs::read(path) else {
        return (Vec::new(), offset);
    };
    let len = bytes.len() as u64;
    let from = usize::try_from(if offset > len { 0 } else { offset }).unwrap_or(0);
    let slice = &bytes[from.min(bytes.len())..];
    let cap = usize::try_from(MAX_PARSE_BYTES).unwrap_or(usize::MAX);
    let capped = &slice[slice.len().saturating_sub(cap)..];
    (capped.to_vec(), len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionStore;
    use crate::Backend;

    /// A store of this test's own, on the same pattern the store's tests use: named for the thread
    /// so two cases never share a database, and wiped first so a previous run's ghost is not read.
    fn store(tag: &str) -> (std::path::PathBuf, SessionStore) {
        let dir = std::env::temp_dir().join(format!(
            "adi-human-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SessionStore::new(&dir);
        (dir, store)
    }

    fn spec(cwd: &Path) -> RunSpec {
        RunSpec {
            cwd: cwd.to_path_buf(),
            path: String::new(),
            env: Vec::new(),
            arguments: Value::Null,
            tools: Vec::new(),
            tool_help: None,
            system_prompt: None,
            workspace_note: None,
            knowledge_note: None,
        }
    }

    /// The seat is empty until somebody is sent to it, occupied while they are there, and empty
    /// again once the turn is stopped. Every listing in the app reads this to decide whether the run
    /// is in flight.
    #[test]
    fn a_seat_is_taken_by_send_and_given_up_by_stop() {
        let (dir, store) = store("seat");
        let record = store
            .create("sim", Backend::HarnessAdi, &dir, "go")
            .expect("a session");
        let session = store.session("sim", &record.id);
        let runner = HumanRunner::new();

        assert!(!runner.is_alive(&session), "nobody has been sent to it yet");
        runner
            .send(&spec(&dir), &session, "go")
            .expect("taking the seat needs no process");
        assert!(runner.is_alive(&session));
        assert!(session.has_started(), "the log exists, so a later send resumes");

        let stopped = runner.stop(&session, Duration::from_secs(1)).expect("stop");
        assert!(stopped.was_running);
        assert!(!stopped.forced, "there is nothing to be rude to");
        assert!(!runner.is_alive(&session));

        // Stopping again finds nothing, which is not an error and not a force either.
        let again = runner.stop(&session, Duration::ZERO).expect("stop");
        assert!(!again.was_running);
        assert!(!again.forced);
    }

    /// A turn typed by a person reads back through the same parser as one printed by the adi loop —
    /// which is what makes the transcript, the settle and the outcome work with no simulated
    /// special case anywhere above this.
    #[test]
    fn a_turn_written_by_hand_reads_back_as_events() {
        let (dir, store) = store("events");
        let record = store
            .create("sim", Backend::HarnessAdi, &dir, "go")
            .expect("a session");
        let session = store.session("sim", &record.id);
        let runner = HumanRunner::new();
        runner.send(&spec(&dir), &session, "go").expect("send");

        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(session.log_path())
            .expect("the log send created");
        adi_events::message(&mut log, "looking at it now");
        adi_events::tool_started(&mut log, "c1", "Bash", &json!({"command": "ls"}));
        adi_events::tool_finished(&mut log, "c1", "Bash", "a.txt\n", true);
        adi_events::answer(&mut log, "one file");

        let batch = runner.events(&session, None).expect("events");
        assert!(matches!(batch.events[0], RunEvent::Step(crate::progress::Step::Message { .. })));
        assert!(matches!(batch.events[1], RunEvent::Step(crate::progress::Step::Tool { .. })));
        assert!(
            matches!(&batch.events[2], RunEvent::Answer { text } if text == "one file"),
            "the answer closes the turn: {:?}",
            batch.events,
        );
    }

    /// It reports itself as a conversation you can answer. A reader draws the reply box from this
    /// *before* anybody types in it, so getting it wrong means a simulated run that looks one-shot.
    #[test]
    fn it_is_a_conversation_and_not_a_terminal() {
        let runner = HumanRunner::new();
        assert_eq!(runner.kind().as_str(), "human");
        assert!(runner.resumes());
        assert!(runner.as_terminal().is_none());
        assert!(runner.emits().message && runner.emits().tool_call);
    }
}
