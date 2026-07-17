//! Backend-agnostic agent run dispatch.
//!
//! The public run API stays here while executor-specific code lives under
//! `backends/<executor>/`. Only the tmux executor is interactive today; future process and
//! harness executors can be added without putting their lifecycle code in this module.

use crate::backends::{process, tmux};
use crate::error::{Error, Result};
use crate::{StoredAgent, StoredAgentManifest};
use std::path::{Path, PathBuf};

pub use tmux::{capture_pane, running_sessions, send_keys, session_name};

/// A successfully launched agent: where it runs and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The engine command that was started, for display.
    pub command: String,
    /// The executor-owned session name (tmux runs only).
    pub session: Option<String>,
    /// The command a human runs to take over the session (tmux runs only).
    pub attach: Option<String>,
    /// The operating-system process id (detached process runs only).
    pub pid: Option<u32>,
    /// The file receiving stdout and stderr (detached process runs only).
    pub log: Option<PathBuf>,
}

/// Whether this manifest has a run adapter today.
#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    match manifest.executor() {
        "tmux" => tmux::is_runnable(manifest),
        "process" => process::is_runnable(manifest),
        _ => false,
    }
}

/// Launch `agent` using the executor named by its backend.
///
/// # Errors
/// [`Error::NotRunnable`] for a backend without an adapter, plus errors from the selected
/// executor.
pub fn launch(agent: &StoredAgent) -> Result<Launch> {
    let sessions_dir = adi_config::Config::open()
        .module("sessions")
        .dir()
        .to_path_buf();
    launch_in(agent, &sessions_dir, "run")
}

/// Stop a registered agent in the standard store using its executor's lifecycle.
///
/// # Errors
/// Returns store or executor-specific lifecycle errors.
pub fn stop(name: &str) -> Result<bool> {
    crate::Agents::open().stop(name)
}

pub(crate) fn launch_in(agent: &StoredAgent, sessions_dir: &Path, message: &str) -> Result<Launch> {
    match agent.manifest.executor() {
        "tmux" => tmux::launch(agent),
        "process" => process::launch(agent, sessions_dir, message),
        _ => Err(Error::NotRunnable(agent.manifest.backend.clone())),
    }
}

pub(crate) fn is_running_in(agent: &StoredAgent, sessions_dir: &Path) -> bool {
    match agent.manifest.executor() {
        "tmux" => tmux::is_running(&agent.name),
        "process" => process::is_running(sessions_dir, &agent.name),
        _ => false,
    }
}

pub(crate) fn stop_in(agent: &StoredAgent, sessions_dir: &Path) -> Result<bool> {
    match agent.manifest.executor() {
        "tmux" => tmux::stop(&agent.name),
        "process" => process::stop(sessions_dir, &agent.name),
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(backend: &str) -> StoredAgentManifest {
        StoredAgentManifest {
            backend: backend.into(),
            ..StoredAgentManifest::default()
        }
    }

    #[test]
    fn only_implemented_backends_are_runnable() {
        assert!(is_runnable(&manifest("tmux:claude")));
        assert!(is_runnable(&manifest("tmux:codex")));
        assert!(is_runnable(&manifest("process:claude")));
        assert!(is_runnable(&manifest("process:codex")));
        for backend in [
            "tmux:unknown",
            "process:unknown",
            "harness:claude-sdk",
            "harness:adi",
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
        assert!(
            matches!(launch(&agent), Err(Error::NotRunnable(backend)) if backend == "harness:adi")
        );
    }
}
