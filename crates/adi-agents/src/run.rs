//! Backend-agnostic run dispatch.

use crate::backend::Backend;
use crate::backends::{harness, process, tmux};
use crate::error::{Error, Result};
use crate::{StoredAgent, StoredAgentManifest};
use std::path::{Path, PathBuf};

pub use tmux::{capture_pane, running_sessions, send_keys, session_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Tmux {
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

/// How much of a detached run's log the live view tails — the last 64 KiB, enough for the tail of
/// a headless `--print` run without streaming an unbounded file to the browser each poll.
pub(crate) const MAX_LOG_TAIL: u64 = 64 * 1024;

/// A read-only snapshot of one run for the live view: the visible output (a tmux pane capture, or the
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
    /// The task the run was launched with.
    pub message: String,
    pub running: bool,
}

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    match &manifest.backend {
        Backend::TmuxClaude | Backend::TmuxCodex => tmux::is_runnable(manifest),
        Backend::ProcessClaude | Backend::ProcessCodex => process::is_runnable(manifest),
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => harness::is_runnable(manifest),
        _ => false,
    }
}

/// Whether a backend runs an interactive session (a tmux pane you type into) rather than a headless,
/// history-keeping run.
fn is_interactive(backend: &Backend) -> bool {
    matches!(backend, Backend::TmuxClaude | Backend::TmuxCodex)
}

/// Launch an agent. `base_dir` is the default working directory a run starts in when the agent
/// defines no explicit `working_dir` of its own — the ADI mono store root, threaded from the store.
pub(crate) fn launch_in(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    message: &str,
) -> Result<Launch> {
    match &agent.manifest.backend {
        Backend::TmuxClaude | Backend::TmuxCodex => tmux::launch(agent, base_dir),
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::launch(agent, sessions_dir, base_dir, message)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::launch(agent, sessions_dir, base_dir, message)
        }
        other => Err(Error::NotRunnable(other.to_string())),
    }
}

/// A headless agent's run history, newest first. Interactive (tmux) backends have no history — their
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

/// A snapshot of one specific detached run, or of the tmux pane for an interactive backend
/// (`run_id` is ignored there — an interactive agent has a single session, not runs).
pub(crate) fn peek_run_in(agent: &StoredAgent, sessions_dir: &Path, run_id: &str) -> Peek {
    match &agent.manifest.backend {
        Backend::TmuxClaude | Backend::TmuxCodex => tmux_peek(agent),
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

/// A name-based snapshot: the tmux pane for interactive backends, or the latest run for headless
/// ones — a convenience for callers that don't track a specific run (the tmux live view).
pub(crate) fn peek_in(agent: &StoredAgent, sessions_dir: &Path) -> Peek {
    if is_interactive(&agent.manifest.backend) {
        return tmux_peek(agent);
    }
    match runs_in(agent, sessions_dir).first() {
        Some(latest) => peek_run_in(agent, sessions_dir, &latest.run_id),
        None => empty_peek(),
    }
}

fn tmux_peek(agent: &StoredAgent) -> Peek {
    let pane = tmux::capture_pane(&agent.name);
    Peek {
        running: pane.is_some(),
        output: pane.unwrap_or_default(),
        attach: format!("tmux attach -t {}", tmux::session_name(&agent.name)),
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

/// Whether the agent has any live run (any headless run still alive, or a live tmux session).
pub(crate) fn is_running_in(agent: &StoredAgent, sessions_dir: &Path) -> bool {
    match &agent.manifest.backend {
        Backend::TmuxClaude | Backend::TmuxCodex => tmux::is_running(&agent.name),
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::any_running(sessions_dir, &agent.name)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::any_running(sessions_dir, &agent.name)
        }
        _ => false,
    }
}

/// Stop one specific detached run, or the tmux session for an interactive backend (`run_id`
/// ignored). Returns whether a live run was found and signalled.
pub(crate) fn stop_run_in(agent: &StoredAgent, sessions_dir: &Path, run_id: &str) -> Result<bool> {
    match &agent.manifest.backend {
        Backend::TmuxClaude | Backend::TmuxCodex => tmux::stop(&agent.name),
        Backend::ProcessClaude | Backend::ProcessCodex => {
            process::stop(sessions_dir, &agent.name, run_id)
        }
        Backend::HarnessClaudeSdk | Backend::HarnessAdi => {
            harness::stop(sessions_dir, &agent.name, run_id)
        }
        _ => Ok(false),
    }
}

/// Stop the agent wholesale: the tmux session, or every live run of a headless agent.
pub(crate) fn stop_in(agent: &StoredAgent, sessions_dir: &Path) -> Result<bool> {
    if is_interactive(&agent.manifest.backend) {
        return tmux::stop(&agent.name);
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
        assert!(is_runnable(&manifest("tmux:claude")));
        assert!(is_runnable(&manifest("tmux:codex")));
        assert!(is_runnable(&manifest("process:claude")));
        assert!(is_runnable(&manifest("process:codex")));
        assert!(is_runnable(&manifest("harness:claude-sdk")));
        for backend in [
            "tmux:unknown",
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
            launch_in(&agent, Path::new("/unused"), Path::new("/unused"), "run"),
            Err(Error::NotRunnable(backend)) if backend == "harness:adi"
        ));
    }
}
