//! The vocabulary a run is described in, and the few things askable without one.
//!
//! This used to be the dispatch layer: fourteen `match`es over [`Backend`], one per verb, each a
//! separate place that had to learn every engine. All of it is gone. [`crate::Agents`] goes through
//! the session store and one runner lookup ([`crate::runner::runner_for`]), so what remains here is
//! the shared vocabulary — [`Launch`], [`Sent`], [`Peek`], [`RunInfo`] — plus the handful of
//! questions that are answered *without* a stored session, and so have nowhere else to live.

use std::path::{Path, PathBuf};

use crate::StoredAgentManifest;
use crate::backend::Backend;
use crate::backends::pty;
use crate::error::{Error, Result};
use crate::runner::{RunSpec, Session, runner_for};

pub use crate::store::Turn;
pub use pty::{running_sessions, session_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Pty {
        command: String,
        session: String,
    },
    Process {
        command: String,
        pid: u32,
        log: PathBuf,
        /// This run's id — its own log/PID slot, independent of every other run of the same agent.
        run_id: String,
    },
}

/// What became of a message said into a conversation. One turn runs at a time, so a message sent
/// while the agent is still answering is not refused — it takes its place in the queue and starts
/// when the current answer lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// It started a turn of its own, right away.
    Started(Launch),
    /// It is waiting: `place` is its 1-based position in the queue.
    Queued { place: usize },
}

/// How much of a detached run's log the live view tails — the last 64 KiB, enough for the tail of
/// a headless `--print` run without streaming an unbounded file to the browser each poll.
pub(crate) const MAX_LOG_TAIL: u64 = 64 * 1024;

/// A read-only snapshot of one run for the live view: the visible output (a pty screen capture, or the
/// tail of a detached run's log — which persists after the run ends), whether it is still live, a
/// human attach/tail hint, and whether the backend is interactive (only an interactive one can be
/// typed into).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peek {
    pub running: bool,
    pub output: String,
    pub attach: String,
    pub interactive: bool,
}

/// One entry in a headless agent's run history. The agent definition is only a template: each Run
/// spawns an independent run from those settings (a fresh dialog, never continuing a prior one),
/// keeps its own log, and several may be live at once. Newest-first ordering is the caller's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInfo {
    pub run_id: String,
    /// Unix milliseconds the run started (encoded in, and recovered from, the run id).
    pub started_at: u64,
    /// Unix milliseconds the run last did anything — the newest change to any file it keeps (its
    /// log, a harness conversation's transcript). Never earlier than `started_at`, so a run whose
    /// files carry no usable timestamp still sorts by when it began.
    pub last_activity: u64,
    /// The task the run was launched with.
    pub message: String,
    pub running: bool,
    /// Whether a reader has hidden this session from the chat rail. Purely a listing preference,
    /// kept in the run's metadata so it survives a reload: a hidden run still runs, still keeps its
    /// log and transcript, and is still returned here — it is up to the *view* to leave it out.
    pub hidden: bool,
}

/// Whether this agent could run at all: something here runs its backend, and that runner accepts
/// its arguments.
///
/// Asked of the runner rather than matched per backend, and answered with **no side effects** — the
/// spec it checks against carries only the stored arguments, since nothing else bears on "would the
/// Run button work". A real launch builds the full spec (cwd, `.bin`, `PATH`, environment), which
/// writes to disk and has no business happening on a page render.
#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    let Some(runner) = runner_for(&manifest.backend) else {
        return false;
    };
    runner
        .check(&RunSpec {
            cwd: PathBuf::new(),
            path: String::new(),
            env: Vec::new(),
            arguments: manifest.arguments_value(),
            tools: Vec::new(),
            system_prompt: None,
            workspace_note: None,
        })
        .is_ok()
}

/// A live pane, addressed by the agent that owns it and nothing else.
///
/// The terminal half of a runner ([`crate::runner::Terminal`]) is reached through a
/// [`Session`], but a pane is keyed by *name* — one per agent, shared with everything else that
/// lists `adi-agent-*` — so the three free functions below can drive it without a stored session to
/// hand over. Nothing is kept: the name is derived from the agent, so there is no state worth
/// writing down and `set_state` says so by discarding it.
#[derive(Debug)]
struct Pane {
    agent: String,
    /// Unused — a terminal writes no log. It is here because [`Session::log_path`] hands out a
    /// borrow, which has to point at something.
    log: PathBuf,
}

impl Session for Pane {
    // A pane has no session id: it is one per agent, and the store never files it.
    #[allow(clippy::unnecessary_literal_bound)]
    fn id(&self) -> &str {
        ""
    }
    fn agent(&self) -> &str {
        &self.agent
    }
    fn has_started(&self) -> bool {
        false
    }
    fn state(&self) -> Option<serde_json::Value> {
        None
    }
    fn set_state(&self, _value: serde_json::Value) -> Result<()> {
        Ok(())
    }
    fn log_path(&self) -> &Path {
        &self.log
    }
}

/// Do something with the pane of `agent_name`, through whichever runner drives terminals.
///
/// The engine is irrelevant here: `pty:claude` and `pty:codex` differ only in the command they open
/// a pane *with*, and typing into one or reading its screen is the same act either way. So this asks
/// one terminal runner and lets [`crate::runner::Runner::as_terminal`] answer for all of them.
fn with_pane<T>(agent_name: &str, f: impl FnOnce(&dyn crate::runner::Terminal, &Pane) -> T) -> Option<T> {
    let runner = runner_for(&Backend::PtyClaude)?;
    let terminal = runner.as_terminal()?;
    let pane = Pane {
        agent: agent_name.to_string(),
        log: PathBuf::new(),
    };
    Some(f(terminal, &pane))
}

/// Type into an agent's live pane.
///
/// # Errors
/// Returns [`Error::NotRunning`] when there is no pane to type into, plus validation and pty errors.
pub fn send_keys(agent_name: &str, text: &str, key: &str) -> Result<()> {
    with_pane(agent_name, |terminal, pane| {
        terminal.send_keys(pane, text, key)
    })
    .unwrap_or_else(|| Err(Error::NotRunning(agent_name.to_string())))
}

/// An agent's visible screen, or `None` when there is nothing to capture.
#[must_use]
pub fn capture_pane(agent_name: &str) -> Option<String> {
    with_pane(agent_name, |terminal, pane| terminal.capture(pane)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(backend: &str) -> StoredAgentManifest {
        StoredAgentManifest {
            backend: Backend::from(backend),
            ..StoredAgentManifest::default()
        }
    }

    #[test]
    fn only_implemented_backends_are_runnable() {
        assert!(is_runnable(&manifest("pty:claude")));
        assert!(is_runnable(&manifest("pty:codex")));
        assert!(is_runnable(&manifest("process:claude")));
        assert!(is_runnable(&manifest("process:codex")));
        assert!(is_runnable(&manifest("harness:claude-sdk")));
        for backend in [
            "pty:unknown",
            "process:unknown",
            "harness:adi",
            "harness:unknown",
            "wasm:loop-script",
            "",
        ] {
            assert!(
                !is_runnable(&manifest(backend)),
                "{backend} must not be runnable yet"
            );
        }
    }

    /// The same refusal [`is_runnable`] reports to the UI, asked of the verb a launch actually
    /// calls. `Agents::run` checks the spec before it appends a turn or spawns anything, so a
    /// backend that cannot run is rejected with nothing written — the two must not drift apart.
    #[test]
    fn an_unconfigured_engine_is_rejected_before_launch() {
        let manifest = manifest("harness:adi");
        let runner = runner_for(&manifest.backend).expect("harness:adi has a runner");
        assert!(matches!(
            runner.check(&RunSpec {
                cwd: PathBuf::new(),
                path: String::new(),
                env: Vec::new(),
                arguments: manifest.arguments_value(),
                tools: Vec::new(),
                system_prompt: None,
                workspace_note: None,
            }),
            Err(Error::NotRunnable(backend)) if backend == "harness:adi"
        ));
    }
}
