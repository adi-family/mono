//! The `harness` executor: an agentic loop ADI drives itself, rather than a vendor CLI that owns
//! its own loop.
//!
//! - `harness:claude-sdk` runs the `claude` CLI headless (a turn-capped, adi-scoped `--print` run),
//!   spawned detached through the shared [`super::detached`] machinery, just like the `process`
//!   executor but under its own `harness/` runtime subdir.
//! - `harness:adi` is ADI's own loop over a chosen model provider. Its arguments are typed and
//!   stored today, but the loop engine that would run it does not exist yet, so it is not runnable.

mod claude_sdk;

use std::path::{Path, PathBuf};

use crate::arguments::HarnessClaudeSdkArguments;
use crate::backend::Backend;
use crate::backends::detached;
use crate::error::{Error, Result};
use crate::run::Launch;
use crate::{StoredAgent, StoredAgentManifest};

const HARNESS_DIR: &str = "harness";

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    engine_run(manifest, "").is_ok()
}

pub fn launch(agent: &StoredAgent, sessions_dir: &Path, message: &str) -> Result<Launch> {
    let (argv, working_dir) = engine_run(&agent.manifest, message)?;
    detached::launch(
        agent,
        sessions_dir,
        HARNESS_DIR,
        &argv,
        working_dir,
        message,
    )
}

/// This agent's run history, newest first.
#[must_use]
pub fn list_runs(sessions_dir: &Path, agent_name: &str) -> Vec<crate::run::RunInfo> {
    detached::list_runs(sessions_dir, HARNESS_DIR, agent_name)
}

/// Whether any run of this agent is still alive.
#[must_use]
pub fn any_running(sessions_dir: &Path, agent_name: &str) -> bool {
    detached::any_running(sessions_dir, HARNESS_DIR, agent_name)
}

/// Whether one specific run is still alive.
#[must_use]
pub fn is_running(sessions_dir: &Path, agent_name: &str, run_id: &str) -> bool {
    detached::is_running(sessions_dir, HARNESS_DIR, agent_name, run_id)
}

/// Stop one specific run.
pub fn stop(sessions_dir: &Path, agent_name: &str, run_id: &str) -> Result<bool> {
    detached::stop(sessions_dir, HARNESS_DIR, agent_name, run_id)
}

/// The tail of one run's log, for the live view.
#[must_use]
pub fn tail_log(sessions_dir: &Path, agent_name: &str, run_id: &str) -> Option<String> {
    detached::tail_log(
        sessions_dir,
        HARNESS_DIR,
        agent_name,
        run_id,
        crate::run::MAX_LOG_TAIL,
    )
}

/// The log path of one run — the `tail -f` target the live view shows.
#[must_use]
pub fn log_path(sessions_dir: &Path, agent_name: &str, run_id: &str) -> PathBuf {
    detached::log_path(sessions_dir, HARNESS_DIR, agent_name, run_id)
}

fn engine_run(
    manifest: &StoredAgentManifest,
    message: &str,
) -> Result<(Vec<String>, Option<String>)> {
    match &manifest.backend {
        Backend::HarnessClaudeSdk => {
            let arguments = manifest.typed_arguments::<HarnessClaudeSdkArguments>()?;
            Ok((claude_sdk::argv(&arguments, message), None))
        }
        // Typed and stored, but its loop engine is future work. Reject at the dispatch boundary so
        // `is_runnable` reads false and `launch` fails cleanly instead of spawning nothing.
        Backend::HarnessAdi => Err(Error::NotRunnable(manifest.backend.to_string())),
        other => Err(Error::NotRunnable(other.to_string())),
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
    fn claude_sdk_is_runnable_and_builds_a_command() {
        let manifest = manifest("harness:claude-sdk");
        assert!(is_runnable(&manifest));
        let (argv, working_dir) = engine_run(&manifest, "go").expect("engine_run");
        assert_eq!(argv.first().map(String::as_str), Some("claude"));
        assert!(argv.iter().any(|a| a == "--print"));
        assert!(working_dir.is_none());
    }

    #[test]
    fn adi_is_typed_but_not_runnable_yet() {
        let manifest = manifest("harness:adi");
        assert!(!is_runnable(&manifest));
        assert!(matches!(
            engine_run(&manifest, "go"),
            Err(Error::NotRunnable(backend)) if backend == "harness:adi"
        ));
    }

    #[test]
    fn unknown_harness_engines_are_not_runnable() {
        assert!(matches!(
            engine_run(&manifest("harness:unknown"), "go"),
            Err(Error::NotRunnable(_))
        ));
    }
}
