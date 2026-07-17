//! Backend-agnostic run dispatch.

use crate::backend::Backend;
use crate::backends::{process, tmux};
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
    },
}

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    match Backend::parse(&manifest.backend) {
        Some(Backend::TmuxClaude | Backend::TmuxCodex) => tmux::is_runnable(manifest),
        Some(Backend::ProcessClaude | Backend::ProcessCodex) => process::is_runnable(manifest),
        _ => false,
    }
}

pub(crate) fn launch_in(agent: &StoredAgent, sessions_dir: &Path, message: &str) -> Result<Launch> {
    match Backend::parse(&agent.manifest.backend) {
        Some(Backend::TmuxClaude | Backend::TmuxCodex) => tmux::launch(agent),
        Some(Backend::ProcessClaude | Backend::ProcessCodex) => {
            process::launch(agent, sessions_dir, message)
        }
        _ => Err(Error::NotRunnable(agent.manifest.backend.clone())),
    }
}

pub(crate) fn is_running_in(agent: &StoredAgent, sessions_dir: &Path) -> bool {
    match Backend::parse(&agent.manifest.backend) {
        Some(Backend::TmuxClaude | Backend::TmuxCodex) => tmux::is_running(&agent.name),
        Some(Backend::ProcessClaude | Backend::ProcessCodex) => {
            process::is_running(sessions_dir, &agent.name)
        }
        _ => false,
    }
}

pub(crate) fn stop_in(agent: &StoredAgent, sessions_dir: &Path) -> Result<bool> {
    match Backend::parse(&agent.manifest.backend) {
        Some(Backend::TmuxClaude | Backend::TmuxCodex) => tmux::stop(&agent.name),
        Some(Backend::ProcessClaude | Backend::ProcessCodex) => {
            process::stop(sessions_dir, &agent.name)
        }
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
        assert!(matches!(
            launch_in(&agent, Path::new("/unused"), "run"),
            Err(Error::NotRunnable(backend)) if backend == "harness:adi"
        ));
    }
}
