//! Tmux session lifecycle.

mod claude;
mod codex;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::arguments::{TmuxClaudeArguments, TmuxCodexArguments};
use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::run::Launch;
use crate::{StoredAgent, StoredAgentManifest};

const SESSION_PREFIX: &str = "adi-agent-";

/// The tmux session name for an agent. Agent names may contain `.` (valid on disk), which tmux
/// treats as a target separator, so dots become dashes here.
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
    session_exists(&session_name(agent_name))
}

/// Session names of every live `adi-agent-*` tmux session. No tmux binary, no running server,
/// or no sessions all read as an empty set.
#[must_use]
pub fn running_sessions() -> BTreeSet<String> {
    let Ok(out) = Command::new(tmux_bin())
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
    else {
        return BTreeSet::new();
    };
    if !out.status.success() {
        return BTreeSet::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|s| s.starts_with(SESSION_PREFIX))
        .map(ToString::to_string)
        .collect()
}

#[must_use]
pub fn capture_pane(agent_name: &str) -> Option<String> {
    let session = session_name(agent_name);
    // The trailing `:` makes this a target-pane. A bare `=name` is only a target-session.
    let out = Command::new(tmux_bin())
        .args(["capture-pane", "-t", &format!("={session}:"), "-p"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

pub fn stop(agent_name: &str) -> Result<bool> {
    let session = session_name(agent_name);
    if !session_exists(&session) {
        return Ok(false);
    }
    run_tmux(&["kill-session", "-t", &format!("={session}")])?;
    Ok(true)
}

/// # Errors
/// Returns validation, missing-session, or tmux errors.
pub fn send_keys(agent_name: &str, text: &str, key: &str) -> Result<()> {
    let session = session_name(agent_name);
    if !session_exists(&session) {
        return Err(Error::NotRunning(agent_name.to_string()));
    }
    let target = format!("={session}:");
    if !text.is_empty() {
        run_tmux(&["send-keys", "-t", &target, "-l", "--", text])?;
    }
    if !key.is_empty() {
        validate_key(key)?;
        run_tmux(&["send-keys", "-t", &target, key])?;
    }
    Ok(())
}

pub fn launch(
    agent: &StoredAgent,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    let argv = engine_argv(&agent.manifest)?;
    let session = session_name(&agent.name);
    if session_exists(&session) {
        return Err(Error::AlreadyRunning(agent.name.clone()));
    }

    let command = argv.join(" ");
    let mut cmd = Command::new(tmux_bin());
    cmd.args(["new-session", "-d", "-s", &session]);
    // Agents should not inherit the launching daemon's working directory; start in the ADI mono
    // store root (`~/.adi/mono`), threaded in as `base_dir`, so a session opened from the app lands
    // in the ADI store rather than $HOME.
    cmd.args(["-c", &base_dir.to_string_lossy()]);
    cmd.arg(shell_command(&argv, bin_dir, secret_env));

    let out = cmd
        .output()
        .map_err(|e| Error::Launch(format!("couldn't spawn tmux: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error::Launch(if stderr.is_empty() {
            format!("tmux exited with {}", out.status)
        } else {
            stderr
        }));
    }

    Ok(Launch::Tmux { command, session })
}

fn engine_argv(manifest: &StoredAgentManifest) -> Result<Vec<String>> {
    match &manifest.backend {
        Backend::TmuxClaude => {
            let arguments = manifest.typed_arguments::<TmuxClaudeArguments>()?;
            Ok(claude::argv(&arguments))
        }
        Backend::TmuxCodex => {
            let arguments = manifest.typed_arguments::<TmuxCodexArguments>()?;
            Ok(codex::argv(&arguments))
        }
        other => Err(Error::NotRunnable(other.to_string())),
    }
}

fn validate_key(key: &str) -> Result<()> {
    let mut chars = key.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidKey(key.to_string()))
    }
}

fn run_tmux(args: &[&str]) -> Result<()> {
    let out = Command::new(tmux_bin())
        .args(args)
        .output()
        .map_err(|e| Error::Tmux(e.to_string()))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(Error::Tmux(if stderr.is_empty() {
        format!("tmux exited with {}", out.status)
    } else {
        stderr
    }))
}

fn shell_command(argv: &[String], bin_dir: Option<&Path>, secret_env: &[(String, String)]) -> String {
    let engine = argv
        .iter()
        .map(|a| sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    // Inject the agent's secrets first, under their literal names — the value is single-quoted,
    // and the name is a validated env identifier, so nothing here can break out of the export.
    // These come before the PATH export so a secret can't shadow PATH.
    let exports = secret_env
        .iter()
        .map(|(k, v)| format!("export {k}={}; ", sh_quote(v)))
        .collect::<String>();
    // Prepend the agent's own `.bin` (its enabled tools) so it can run them by name, ahead of
    // the standard search dirs. The path lives under the ADI store, which has no shell-special
    // characters, so it's embedded directly into the double-quoted export.
    let prefix = bin_dir
        .map(|d| format!("{}:", d.display()))
        .unwrap_or_default();
    format!(
        "{exports}export PATH=\"{prefix}$HOME/.local/bin:$HOME/bin:/opt/homebrew/bin:/usr/local/bin:$PATH\"; \
         {engine}; status=$?; \
         if [ \"$status\" -ne 0 ]; then \
         printf '\\n[adi] agent exited with status %s - press enter to close\\n' \"$status\"; \
         read _; fi"
    )
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn session_exists(session: &str) -> bool {
    Command::new(tmux_bin())
        .args(["has-session", "-t", &format!("={session}")])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Prefer absolute paths because the app runs under launchd's minimal PATH.
fn tmux_bin() -> &'static str {
    [
        "/opt/homebrew/bin/tmux",
        "/usr/local/bin/tmux",
        "/usr/bin/tmux",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).exists())
    .unwrap_or("tmux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_are_prefixed_and_tmux_safe() {
        assert_eq!(session_name("athz-solver"), "adi-agent-athz-solver");
        assert_eq!(session_name("a.b"), "adi-agent-a-b");
    }

    #[test]
    fn unknown_tmux_engines_are_not_runnable() {
        let manifest = StoredAgentManifest {
            backend: "tmux:unknown".into(),
            ..StoredAgentManifest::default()
        };
        assert!(matches!(engine_argv(&manifest), Err(Error::NotRunnable(_))));
    }

    #[test]
    fn key_names_are_single_tmux_tokens() {
        for key in ["Enter", "Escape", "Up", "Down", "Tab", "C-c", "F5", "1"] {
            assert!(validate_key(key).is_ok(), "{key} should be valid");
        }
        for key in ["", "-l", "--", "Enter Escape", "C c", "'"] {
            assert!(
                matches!(validate_key(key), Err(Error::InvalidKey(_))),
                "{key:?} should be rejected"
            );
        }
    }

    #[test]
    fn shell_command_quotes_arguments_for_sh() {
        let cmd = shell_command(
            &[
                "claude".into(),
                "--append-system-prompt".into(),
                "don't".into(),
            ],
            None,
            &[],
        );
        assert!(
            cmd.contains("'claude' '--append-system-prompt' 'don'\\''t'"),
            "{cmd}"
        );
        assert!(cmd.contains("read _"));
    }

    #[test]
    fn shell_command_prepends_the_agent_bin_dir_to_path() {
        let cmd = shell_command(
            &["claude".into()],
            Some(Path::new("/store/tools/.agent-bin/solver")),
            &[],
        );
        assert!(
            cmd.contains("export PATH=\"/store/tools/.agent-bin/solver:$HOME/.local/bin:"),
            "{cmd}"
        );
    }

    #[test]
    fn shell_command_exports_secrets_safely_before_path() {
        let secrets = vec![
            ("API_KEY".to_string(), "s3 cr'et".to_string()),
            ("TOKEN".to_string(), "abc".to_string()),
        ];
        let cmd = shell_command(&["claude".into()], None, &secrets);
        // Value is single-quoted with the embedded quote escaped; exports precede the PATH line.
        assert!(cmd.contains("export API_KEY='s3 cr'\\''et'; "), "{cmd}");
        assert!(cmd.contains("export TOKEN='abc'; "), "{cmd}");
        let api_at = cmd.find("export API_KEY").expect("api export");
        let path_at = cmd.find("export PATH=").expect("path export");
        assert!(api_at < path_at, "secrets must export before PATH: {cmd}");
    }
}
