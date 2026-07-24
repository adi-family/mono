//! Interactive pty session lifecycle, driven through the in-process [`adi_pty`] session manager.

mod claude;
mod codex;

use std::collections::BTreeSet;
use std::path::Path;

use crate::arguments::{PtyClaudeArguments, PtyCodexArguments};
use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::run::Launch;
use crate::{StoredAgent, StoredAgentManifest};

const SESSION_PREFIX: &str = "adi-agent-";

/// The pty session name for an agent. Agent names may contain `.` (valid on disk); dots become
/// dashes so a session name stays a single flat token.
#[must_use]
pub fn session_name(agent_name: &str) -> String {
    format!("{SESSION_PREFIX}{}", agent_name.replace('.', "-"))
}

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    engine_argv(manifest).is_ok()
}

#[must_use]
pub fn is_running(agent_name: &str) -> bool {
    adi_pty::is_running(&session_name(agent_name))
}

/// Session names of every live `adi-agent-*` pty session.
#[must_use]
pub fn running_sessions() -> BTreeSet<String> {
    adi_pty::running(SESSION_PREFIX)
}

#[must_use]
pub fn capture_pane(agent_name: &str) -> Option<String> {
    adi_pty::capture(&session_name(agent_name))
}

pub fn stop(agent_name: &str) -> Result<bool> {
    adi_pty::stop(&session_name(agent_name)).map_err(|e| Error::Session(e.to_string()))
}

/// # Errors
/// Returns validation, missing-session, or pty errors.
pub fn send_keys(agent_name: &str, text: &str, key: &str) -> Result<()> {
    if !is_running(agent_name) {
        return Err(Error::NotRunning(agent_name.to_string()));
    }
    adi_pty::send_keys(&session_name(agent_name), text, key).map_err(|e| match e {
        adi_pty::Error::InvalidKey(k) => Error::InvalidKey(k),
        other => Error::Session(other.to_string()),
    })
}

pub fn launch(
    agent: &StoredAgent,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    let argv = engine_argv(&agent.manifest)?;
    let session = session_name(&agent.name);
    if adi_pty::is_running(&session) {
        return Err(Error::AlreadyRunning(agent.name.clone()));
    }

    // Inject the agent's secrets under their literal names, then the augmented PATH so its own
    // `.bin` and the standard tool dirs resolve even under launchd's minimal environment.
    let mut env = secret_env.to_vec();
    env.push(("PATH".into(), augmented_path(bin_dir)));

    adi_pty::launch(&session, &argv, base_dir, &env).map_err(|e| Error::Launch(e.to_string()))?;
    Ok(Launch::Pty { command: argv.join(" "), session })
}

fn engine_argv(manifest: &StoredAgentManifest) -> Result<Vec<String>> {
    match &manifest.backend {
        Backend::PtyClaude => {
            let arguments = manifest.typed_arguments::<PtyClaudeArguments>()?;
            Ok(claude::argv(&arguments))
        }
        Backend::PtyCodex => {
            let arguments = manifest.typed_arguments::<PtyCodexArguments>()?;
            Ok(codex::argv(&arguments))
        }
        other => Err(Error::NotRunnable(other.to_string())),
    }
}

/// Build the run's `PATH`: the agent's own `.bin` first (so it can invoke its enabled tools by
/// name), then — on unix — the standard tool dirs the app's minimal launchd `PATH` misses, then the
/// current `PATH`. Joined with the platform separator, so it does the right thing on Windows too.
fn augmented_path(bin_dir: Option<&Path>) -> String {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(bin) = bin_dir {
        dirs.push(bin.to_path_buf());
    }
    #[cfg(unix)]
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }
    #[cfg(unix)]
    {
        dirs.push(std::path::PathBuf::from("/opt/homebrew/bin"));
        dirs.push(std::path::PathBuf::from("/usr/local/bin"));
    }
    if let Some(existing) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(dirs)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_are_prefixed_and_scoped() {
        assert_eq!(session_name("athz-solver"), "adi-agent-athz-solver");
        assert_eq!(session_name("a.b"), "adi-agent-a-b");
    }

    #[test]
    fn unknown_engines_are_not_runnable() {
        let manifest = StoredAgentManifest {
            backend: "pty:unknown".into(),
            ..StoredAgentManifest::default()
        };
        assert!(matches!(engine_argv(&manifest), Err(Error::NotRunnable(_))));
    }
}
