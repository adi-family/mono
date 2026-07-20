//! The `process` executor: a vendor CLI (`claude --print` / `codex exec`) run headless as a
//! detached subprocess. The generic detached-process lifecycle lives in [`super::detached`]; this
//! module only builds the engine command and pins the runtime subdir.

mod claude;
mod codex;

use std::path::{Path, PathBuf};

use crate::arguments::{ProcessClaudeArguments, ProcessCodexArguments};
use crate::backend::Backend;
use crate::backends::detached;
use crate::error::{Error, Result};
use crate::run::Launch;
use crate::{StoredAgent, StoredAgentManifest};

const PROCESS_DIR: &str = "process";

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    engine_run(manifest, "").is_ok()
}

pub fn launch(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    message: &str,
) -> Result<Launch> {
    let (argv, working_dir) = engine_run(&agent.manifest, message)?;
    detached::launch(
        agent,
        sessions_dir,
        base_dir,
        PROCESS_DIR,
        &argv,
        working_dir,
        message,
    )
}

/// This agent's run history, newest first.
#[must_use]
pub fn list_runs(sessions_dir: &Path, agent_name: &str) -> Vec<crate::run::RunInfo> {
    detached::list_runs(sessions_dir, PROCESS_DIR, agent_name)
}

/// Whether any run of this agent is still alive.
#[must_use]
pub fn any_running(sessions_dir: &Path, agent_name: &str) -> bool {
    detached::any_running(sessions_dir, PROCESS_DIR, agent_name)
}

/// Whether one specific run is still alive.
#[must_use]
pub fn is_running(sessions_dir: &Path, agent_name: &str, run_id: &str) -> bool {
    detached::is_running(sessions_dir, PROCESS_DIR, agent_name, run_id)
}

/// Stop one specific run.
pub fn stop(sessions_dir: &Path, agent_name: &str, run_id: &str) -> Result<bool> {
    detached::stop(sessions_dir, PROCESS_DIR, agent_name, run_id)
}

/// The tail of one run's log, for the live view.
#[must_use]
pub fn tail_log(sessions_dir: &Path, agent_name: &str, run_id: &str) -> Option<String> {
    detached::tail_log(
        sessions_dir,
        PROCESS_DIR,
        agent_name,
        run_id,
        crate::run::MAX_LOG_TAIL,
    )
}

/// The log path of one run — the `tail -f` target the live view shows.
#[must_use]
pub fn log_path(sessions_dir: &Path, agent_name: &str, run_id: &str) -> PathBuf {
    detached::log_path(sessions_dir, PROCESS_DIR, agent_name, run_id)
}

fn engine_run(
    manifest: &StoredAgentManifest,
    message: &str,
) -> Result<(Vec<String>, Option<String>)> {
    match &manifest.backend {
        Backend::ProcessClaude => {
            let arguments = manifest.typed_arguments::<ProcessClaudeArguments>()?;
            let working_dir = arguments.working_dir.clone();
            Ok((claude::argv(&arguments, message), working_dir))
        }
        Backend::ProcessCodex => {
            let arguments = manifest.typed_arguments::<ProcessCodexArguments>()?;
            let working_dir = arguments.working_dir.clone();
            Ok((codex::argv(&arguments, message), working_dir))
        }
        other => Err(Error::NotRunnable(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_process_engines_are_not_runnable() {
        let manifest = StoredAgentManifest {
            backend: "process:unknown".into(),
            ..StoredAgentManifest::default()
        };
        assert!(matches!(
            engine_run(&manifest, "run"),
            Err(Error::NotRunnable(_))
        ));
    }
}
