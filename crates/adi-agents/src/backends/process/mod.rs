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
    engine_run(manifest, "", None).is_ok()
}

pub fn launch(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    run_path: &str,
    message: &str,
    run_env: &[(String, String)],
) -> Result<Launch> {
    let argv = engine_run(&agent.manifest, message, base_dir.to_str())?;
    detached::launch(
        agent,
        sessions_dir,
        base_dir,
        run_path,
        PROCESS_DIR,
        &argv,
        message,
        run_env,
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

/// Delete one run: stop it if it is still live, then remove its log and metadata.
pub fn delete(sessions_dir: &Path, agent_name: &str, run_id: &str) -> Result<bool> {
    detached::delete(sessions_dir, PROCESS_DIR, agent_name, run_id)
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

/// Build the engine's command. `workspace` is the run's already-resolved directory: the child is
/// spawned there either way, and Codex is additionally *told* it (`--cd`), because that directory is
/// what scopes its sandbox rather than merely being where it happens to start. `None` builds a
/// command for inspection only ([`is_runnable`]), where no run exists to have a directory.
fn engine_run(
    manifest: &StoredAgentManifest,
    message: &str,
    workspace: Option<&str>,
) -> Result<Vec<String>> {
    match &manifest.backend {
        Backend::ProcessClaude => {
            let arguments = manifest.typed_arguments::<ProcessClaudeArguments>()?;
            Ok(claude::argv(&arguments, message))
        }
        Backend::ProcessCodex => {
            let arguments = manifest.typed_arguments::<ProcessCodexArguments>()?;
            Ok(codex::argv(&arguments, message, workspace))
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
            engine_run(&manifest, "run", None),
            Err(Error::NotRunnable(_))
        ));
    }
}
