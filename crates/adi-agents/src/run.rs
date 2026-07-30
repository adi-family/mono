//! Backend-agnostic run dispatch.

use crate::backend::Backend;
use crate::backends::{harness, process, pty};
use crate::error::{Error, Result};
use crate::{StoredAgent, StoredAgentManifest};
use std::path::{Path, PathBuf};

pub use harness::Turn;
pub use pty::{capture_pane, running_sessions, send_keys, session_name};

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
}

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    match &manifest.backend {
        Backend::PtyClaude | Backend::PtyCodex => pty::is_runnable(manifest),
        Backend::ProcessClaude | Backend::ProcessCodex => process::is_runnable(manifest),
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => harness::is_runnable(manifest),
        _ => false,
    }
}

/// Whether a backend runs an interactive session (a pty screen you type into) rather than a headless,
/// history-keeping run.
fn is_interactive(backend: &Backend) -> bool {
    matches!(backend, Backend::PtyClaude | Backend::PtyCodex)
}

/// Launch an agent. `base_dir` is the default working directory a run starts in when the agent
/// defines no explicit `working_dir` of its own — the ADI mono store root, threaded from the store.
/// `run_path` is the already-assembled `PATH` the run resolves commands on — its own `.bin` of
/// enabled tools, the dirs its manifest declares, then the standard ones (see `launch::run_path`).
pub(crate) fn launch_in(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    run_path: &str,
    message: &str,
    run_env: &[(String, String)],
) -> Result<Launch> {
    match &agent.manifest.backend {
        Backend::PtyClaude | Backend::PtyCodex => {
            pty::launch(agent, base_dir, run_path, run_env)
        }
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::launch(agent, sessions_dir, base_dir, run_path, message, run_env)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::launch(agent, sessions_dir, base_dir, run_path, message, run_env)
        }
        other => Err(Error::NotRunnable(other.to_string())),
    }
}

/// The live detached runs of every agent that has any, keyed by agent name — the raw material both
/// concurrency caps are computed from (the global one sums it; the per-project one sums the agents
/// filed under that project).
///
/// Read from PID files, so a run started by another process (the CLI, a trigger's `adi-agents run`)
/// counts too. Pty sessions are not here: they live inside the process that opened them and keep no
/// run directory to name an agent from — see [`running_sessions`], which the caller matches against
/// the agents it already holds.
pub(crate) fn running_by_agent_in(sessions_dir: &Path) -> std::collections::BTreeMap<String, usize> {
    let mut by_agent = process::running_by_agent(sessions_dir);
    for (agent, live) in harness::running_by_agent(sessions_dir) {
        *by_agent.entry(agent).or_default() += live;
    }
    by_agent
}

/// A headless agent's run history, newest first. Interactive (pty) backends have no history — their
/// live session *is* the run — so this is empty for them.
pub(crate) fn runs_in(agent: &StoredAgent, sessions_dir: &Path) -> Vec<RunInfo> {
    match &agent.manifest.backend {
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::list_runs(sessions_dir, &agent.name)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::list_runs(sessions_dir, &agent.name)
        }
        _ => Vec::new(),
    }
}

/// A snapshot of one specific detached run, or of the pty screen for an interactive backend
/// (`run_id` is ignored there — an interactive agent has a single session, not runs).
pub(crate) fn peek_run_in(agent: &StoredAgent, sessions_dir: &Path, run_id: &str) -> Peek {
    match &agent.manifest.backend {
        Backend::PtyClaude | Backend::PtyCodex => pty_peek(agent),
        Backend::ProcessClaude | Backend::ProcessCodex => detached_peek(
            process::is_running(sessions_dir, &agent.name, run_id),
            process::tail_log(sessions_dir, &agent.name, run_id),
            &process::log_path(sessions_dir, &agent.name, run_id),
        ),
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => detached_peek(
            harness::is_running(sessions_dir, &agent.name, run_id),
            harness::tail_log(sessions_dir, &agent.name, run_id),
            &harness::log_path(sessions_dir, &agent.name, run_id),
        ),
        _ => empty_peek(),
    }
}

/// A name-based snapshot: the pty screen for interactive backends, or the latest run for headless
/// ones — a convenience for callers that don't track a specific run (the pty live view).
pub(crate) fn peek_in(agent: &StoredAgent, sessions_dir: &Path) -> Peek {
    if is_interactive(&agent.manifest.backend) {
        return pty_peek(agent);
    }
    match runs_in(agent, sessions_dir).first() {
        Some(latest) => peek_run_in(agent, sessions_dir, &latest.run_id),
        None => empty_peek(),
    }
}

fn pty_peek(agent: &StoredAgent) -> Peek {
    Peek {
        // A pty session lives only in-process, so liveness is the child's state — not whether a
        // capture exists (a dead session keeps its final screen capturable).
        running: pty::is_running(&agent.name),
        output: pty::capture_pane(&agent.name).unwrap_or_default(),
        // A pty session has no external attach command; it is viewed only in the control panel.
        attach: String::new(),
        interactive: true,
    }
}

fn detached_peek(running: bool, output: Option<String>, log: &Path) -> Peek {
    Peek {
        running,
        output: output.unwrap_or_default(),
        attach: format!("tail -f {}", log.display()),
        interactive: false,
    }
}

fn empty_peek() -> Peek {
    Peek {
        running: false,
        output: String::new(),
        attach: String::new(),
        interactive: false,
    }
}

/// Whether the agent has any live run (any headless run still alive, or a live pty session).
pub(crate) fn is_running_in(agent: &StoredAgent, sessions_dir: &Path) -> bool {
    match &agent.manifest.backend {
        Backend::PtyClaude | Backend::PtyCodex => pty::is_running(&agent.name),
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::any_running(sessions_dir, &agent.name)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::any_running(sessions_dir, &agent.name)
        }
        _ => false,
    }
}

/// Stop one specific detached run, or the pty session for an interactive backend (`run_id`
/// ignored). Returns whether a live run was found and signalled.
pub(crate) fn stop_run_in(agent: &StoredAgent, sessions_dir: &Path, run_id: &str) -> Result<bool> {
    match &agent.manifest.backend {
        Backend::PtyClaude | Backend::PtyCodex => pty::stop(&agent.name),
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::stop(sessions_dir, &agent.name, run_id)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::stop(sessions_dir, &agent.name, run_id)
        }
        _ => Ok(false),
    }
}

/// Delete one specific run of a headless agent, removing it and everything it kept. Interactive
/// (pty) backends keep no run history, so there is nothing there to delete.
pub(crate) fn delete_run_in(
    agent: &StoredAgent,
    sessions_dir: &Path,
    run_id: &str,
) -> Result<bool> {
    match &agent.manifest.backend {
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::delete(sessions_dir, &agent.name, run_id)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::delete(sessions_dir, &agent.name, run_id)
        }
        other => Err(Error::Unsupported(format!(
            "backend {other} keeps no run history, so it has no run to delete"
        ))),
    }
}

/// Say something into one of an agent's conversations: start the next turn, or queue the message
/// behind the answer still in flight — or, when `may_start` is false, behind the run cap. Only
/// harness backends keep conversations; anything else has no thread to continue.
pub(crate) fn reply_in(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    run_path: &str,
    conv_id: &str,
    message: &str,
    run_env: &[(String, String)],
    may_start: bool,
) -> Result<Sent> {
    match &agent.manifest.backend {
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => harness::reply(
            agent,
            sessions_dir,
            base_dir,
            run_path,
            conv_id,
            message,
            run_env,
            may_start,
        ),
        other => Err(Error::Unsupported(format!(
            "backend {other} isn't answerable — only harness backends keep conversations you can reply to"
        ))),
    }
}

/// Start a conversation's next queued message when it is idle and one is waiting. Only harness
/// backends have a queue; for anything else there is nothing to advance.
pub(crate) fn advance_in(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    run_path: &str,
    conv_id: &str,
    run_env: &[(String, String)],
) -> Option<(String, Launch)> {
    match &agent.manifest.backend {
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::advance(agent, sessions_dir, base_dir, run_path, conv_id, run_env)
        }
        _ => None,
    }
}

/// The directory a conversation was started in, for the caller to re-enter on every later turn.
/// `None` when the backend keeps no conversation, or when this one predates the recorded field —
/// see [`harness::pinned_dir`] for why the fallback is the store root rather than a fresh resolve.
pub(crate) fn conversation_dir_in(
    agent: &StoredAgent,
    sessions_dir: &Path,
    conv_id: &str,
) -> Option<PathBuf> {
    match &agent.manifest.backend {
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::pinned_dir(sessions_dir, &agent.name, conv_id)
        }
        _ => None,
    }
}

/// Whether [`advance_in`] would start anything — the cheap gate a poll asks before paying for a
/// launch context.
pub(crate) fn can_advance_in(agent: &StoredAgent, sessions_dir: &Path, conv_id: &str) -> bool {
    match &agent.manifest.backend {
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::can_advance(sessions_dir, &agent.name, conv_id)
        }
        _ => false,
    }
}

/// Drop one queued message of a conversation, by its position in the queue.
pub(crate) fn unqueue_in(
    agent: &StoredAgent,
    sessions_dir: &Path,
    conv_id: &str,
    index: usize,
) -> bool {
    match &agent.manifest.backend {
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::unqueue(sessions_dir, &agent.name, conv_id, index)
        }
        _ => false,
    }
}

/// Run one `adi` conversation turn — read the transcript, call the provider, return the answer.
/// See [`crate::Agents::run_adi_turn`]. Only the `adi` harness engine has a loop to run.
pub(crate) fn adi_turn_in(agent: &StoredAgent, sessions_dir: &Path, conv_id: &str) -> Result<String> {
    match &agent.manifest.backend {
        Backend::HarnessAdi => harness::run_adi_turn(agent, sessions_dir, conv_id),
        other => Err(Error::Unsupported(format!(
            "backend {other} has no adi loop to run"
        ))),
    }
}

/// A run's turns, oldest first. For a harness backend this is the answerable conversation; for a
/// one-shot process run it is the task and its single parsed answer (so both render as the same
/// progress feed). Interactive (pty) and wasm backends keep no turn transcript.
pub(crate) fn transcript_in(agent: &StoredAgent, sessions_dir: &Path, conv_id: &str) -> Vec<Turn> {
    match &agent.manifest.backend {
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::transcript(sessions_dir, &agent.name, conv_id, &agent.manifest.backend)
        }
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process_run_turns(agent, sessions_dir, conv_id)
        }
        _ => Vec::new(),
    }
}

/// Synthesize a one-shot process run as a two-turn transcript: the task it was launched with (a user
/// turn) and its parsed output (a single assistant turn, still `pending` while the run is live). This
/// lets a headless run render through the same progress feed as a conversation.
fn process_run_turns(agent: &StoredAgent, sessions_dir: &Path, run_id: &str) -> Vec<Turn> {
    let task = process::list_runs(sessions_dir, &agent.name)
        .into_iter()
        .find(|r| r.run_id == run_id)
        .map(|r| r.message)
        .unwrap_or_default();
    let running = process::is_running(sessions_dir, &agent.name, run_id);
    let log = read_capped(&process::log_path(sessions_dir, &agent.name, run_id));
    let content = crate::progress::parse(&agent.manifest.backend, &log);

    let mut turns = Vec::new();
    if !task.trim().is_empty() {
        turns.push(Turn {
            role: "user".to_string(),
            text: task,
            at: 0,
            pending: false,
            queued: false,
            steps: Vec::new(),
            metrics: None,
        });
    }
    turns.push(Turn {
        role: "assistant".to_string(),
        text: content.text,
        at: 0,
        pending: running,
        queued: false,
        steps: content.steps,
        metrics: content.metrics,
    });
    turns
}

/// Read a log file whole, up to the progress parse cap.
fn read_capped(path: &Path) -> Vec<u8> {
    use std::io::Read as _;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = file.take(crate::progress::MAX_PARSE_BYTES).read_to_end(&mut buf);
    buf
}

/// Stop the agent wholesale: the pty session, or every live run of a headless agent.
pub(crate) fn stop_in(agent: &StoredAgent, sessions_dir: &Path) -> Result<bool> {
    if is_interactive(&agent.manifest.backend) {
        return pty::stop(&agent.name);
    }
    let mut stopped = false;
    for run in runs_in(agent, sessions_dir) {
        if run.running && stop_run_in(agent, sessions_dir, &run.run_id)? {
            stopped = true;
        }
    }
    Ok(stopped)
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

    #[test]
    fn an_unimplemented_executor_is_rejected_before_launch() {
        let agent = StoredAgent {
            name: "planner".into(),
            manifest: manifest("harness:adi"),
        };
        assert!(matches!(
            launch_in(&agent, Path::new("/unused"), Path::new("/unused"), "", "run", &[]),
            Err(Error::NotRunnable(backend)) if backend == "harness:adi"
        ));
    }
}
