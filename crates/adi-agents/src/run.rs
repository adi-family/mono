//! Launching an agent — the first slice of the run/orchestration layer from docs/adi-agents.md.
//! Only tmux-backed backends (`tmux:claude`, `tmux:codex`) run today: the vendor CLI is started
//! detached inside a tmux session named `adi-agent-<name>`, and the caller attaches to observe
//! (`tmux attach -t adi-agent-<name>`). Every other executor returns [`Error::NotRunnable`] until
//! its adapter exists. Session persistence, event streams, and command-scope enforcement are
//! still future work.

use std::collections::BTreeSet;
use std::process::Command;

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::AgentManifest;

/// Prefix of every tmux session this launcher owns.
const SESSION_PREFIX: &str = "adi-agent-";

/// A successfully launched agent: where it runs and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The tmux session name (`adi-agent-<name>`).
    pub session: String,
    /// The engine command that was started inside the session, for display.
    pub command: String,
    /// The command a human runs to observe the agent: `tmux attach -t <session>`.
    pub attach: String,
}

/// The tmux session name for an agent. Agent names may contain `.` (valid on disk), which tmux
/// forbids in session names — it's a target separator — so dots become dashes here.
#[must_use]
pub fn session_name(agent_name: &str) -> String {
    format!("{SESSION_PREFIX}{}", agent_name.replace('.', "-"))
}

/// Whether this manifest's backend has a run adapter today (tmux executors only).
#[must_use]
pub fn is_runnable(manifest: &AgentManifest) -> bool {
    engine_argv(manifest).is_ok()
}

/// Session names of every live `adi-agent-*` tmux session. No tmux binary, no running server,
/// or no sessions all read as an empty set — "nothing running" is the honest default.
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

/// A read-only snapshot of a running agent's visible tmux pane — the text `tmux attach` would
/// show. `None` when the agent has no live session (or tmux isn't available), so a caller can't
/// confuse "not running" with "running but blank".
#[must_use]
pub fn capture_pane(agent_name: &str) -> Option<String> {
    let session = session_name(agent_name);
    // The trailing `:` makes this a valid target-pane (exact-match session, default window and
    // pane). A bare `=name` is only a target-session — capture-pane rejects it.
    let out = Command::new(tmux_bin())
        .args(["capture-pane", "-t", &format!("={session}:"), "-p"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Stop a running agent by killing its tmux session. Returns whether a live session was found
/// and killed — idempotent, so stopping an already-stopped agent is a successful `false`.
///
/// # Errors
/// [`Error::Tmux`] when the kill itself fails on an existing session.
pub fn stop(agent_name: &str) -> Result<bool> {
    let session = session_name(agent_name);
    if !session_exists(&session) {
        return Ok(false);
    }
    run_tmux(&["kill-session", "-t", &format!("={session}")])?;
    Ok(true)
}

/// Send input to a running agent's tmux session — the interactive half of the live view:
/// `text` is typed literally (no key-name interpretation), then `key` (a tmux key name like
/// `Enter`, `Escape`, `Up`, `C-c`) is pressed. Either part may be empty.
///
/// # Errors
/// [`Error::NotRunning`] when the agent has no live session, [`Error::InvalidKey`] for a key
/// that isn't a single tmux key token, or [`Error::Tmux`] when tmux itself fails.
pub fn send_keys(agent_name: &str, text: &str, key: &str) -> Result<()> {
    let session = session_name(agent_name);
    if !session_exists(&session) {
        return Err(Error::NotRunning(agent_name.to_string()));
    }
    let target = format!("={session}:");
    if !text.is_empty() {
        // `-l` sends the text literally; `--` keeps text starting with `-` out of the flags.
        run_tmux(&["send-keys", "-t", &target, "-l", "--", text])?;
    }
    if !key.is_empty() {
        validate_key(key)?;
        run_tmux(&["send-keys", "-t", &target, key])?;
    }
    Ok(())
}

/// A key must be one tmux key token (`Enter`, `Escape`, `Up`, `C-c`, `F5`, …): alphanumeric
/// with inner dashes. The leading character is kept strict so a key can never parse as a tmux
/// flag.
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

/// Run one tmux command, mapping a spawn failure or non-zero exit to [`Error::Tmux`].
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

/// Launch `agent` in its backend: start the engine CLI detached in a fresh tmux session.
///
/// # Errors
/// [`Error::NotRunnable`] for a backend without an adapter, [`Error::AlreadyRunning`] when the
/// agent's session already exists, or [`Error::Launch`] when tmux itself fails.
pub fn launch(agent: &Agent) -> Result<Launch> {
    let argv = engine_argv(&agent.manifest)?;
    let session = session_name(&agent.name);
    if session_exists(&session) {
        return Err(Error::AlreadyRunning(agent.name.clone()));
    }

    let command = argv.join(" ");
    let mut cmd = Command::new(tmux_bin());
    cmd.args(["new-session", "-d", "-s", &session]);
    // Agents shouldn't inherit the daemon's working directory; start in the user's home.
    if let Ok(home) = std::env::var("HOME") {
        cmd.args(["-c", &home]);
    }
    cmd.arg(shell_command(&argv));

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

    Ok(Launch {
        attach: format!("tmux attach -t {session}"),
        session,
        command,
    })
}

/// The engine argv for a manifest, or [`Error::NotRunnable`] when its backend has no adapter.
/// Deliberately minimal for the first slice: the launch honors the model, the Claude permission
/// mode, and the system prompt; the finer-grained `extra` knobs land with the real executor
/// adapters.
fn engine_argv(manifest: &AgentManifest) -> Result<Vec<String>> {
    let mut argv: Vec<String> = match manifest.backend.as_str() {
        "tmux:claude" => {
            let mut argv = vec!["claude".to_string()];
            if let Some(mode) = &manifest.permission_mode {
                argv.extend(["--permission-mode".to_string(), mode.clone()]);
            }
            if !manifest.system_prompt.trim().is_empty() {
                argv.extend([
                    "--append-system-prompt".to_string(),
                    manifest.system_prompt.clone(),
                ]);
            }
            argv
        }
        "tmux:codex" => {
            let mut argv = vec!["codex".to_string()];
            if let Some(sandbox) = manifest.extra.get("sandbox").filter(|s| !s.is_empty()) {
                argv.extend(["--sandbox".to_string(), sandbox.clone()]);
            }
            argv
        }
        other => return Err(Error::NotRunnable(other.to_string())),
    };
    if let Some(model) = &manifest.model {
        argv.extend(["--model".to_string(), model.clone()]);
    }
    Ok(argv)
}

/// Wrap the engine argv into the single `sh -c` command line tmux runs. The PATH export makes
/// the engine resolvable even when the launching process runs under launchd's minimal PATH
/// (Homebrew and per-user bin dirs aren't there); the exit-status tail keeps the pane open on a
/// failed start so there's an error to read instead of a silently vanished session.
fn shell_command(argv: &[String]) -> String {
    let engine = argv
        .iter()
        .map(|a| sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "export PATH=\"$HOME/.local/bin:$HOME/bin:/opt/homebrew/bin:/usr/local/bin:$PATH\"; \
         {engine}; status=$?; \
         if [ \"$status\" -ne 0 ]; then \
         printf '\\n[adi] agent exited with status %s - press enter to close\\n' \"$status\"; \
         read _; fi"
    )
}

/// Single-quote `s` for `sh -c`, escaping embedded quotes as `'\''`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Whether a tmux session with exactly this name exists (`=` pins tmux to an exact match
/// instead of its default prefix matching).
fn session_exists(session: &str) -> bool {
    Command::new(tmux_bin())
        .args(["has-session", "-t", &format!("={session}")])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The tmux binary: the first well-known absolute location that exists, falling back to a PATH
/// lookup. The absolute candidates matter because the app daemon runs under launchd, whose PATH
/// misses Homebrew.
fn tmux_bin() -> &'static str {
    ["/opt/homebrew/bin/tmux", "/usr/local/bin/tmux", "/usr/bin/tmux"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("tmux")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(backend: &str) -> AgentManifest {
        AgentManifest {
            backend: backend.into(),
            ..AgentManifest::default()
        }
    }

    #[test]
    fn session_names_are_prefixed_and_tmux_safe() {
        assert_eq!(session_name("athz-solver"), "adi-agent-athz-solver");
        // tmux forbids '.' in session names; it must be mangled, not passed through.
        assert_eq!(session_name("a.b"), "adi-agent-a-b");
    }

    #[test]
    fn only_tmux_backends_are_runnable() {
        assert!(is_runnable(&manifest("tmux:claude")));
        assert!(is_runnable(&manifest("tmux:codex")));
        for backend in ["process:claude", "process:codex", "harness:claude-sdk", "harness:adi", ""] {
            assert!(!is_runnable(&manifest(backend)), "{backend} must not be runnable yet");
            assert!(matches!(
                engine_argv(&manifest(backend)),
                Err(Error::NotRunnable(_))
            ));
        }
    }

    #[test]
    fn claude_argv_honors_model_permission_mode_and_prompt() {
        let mut m = manifest("tmux:claude");
        m.model = Some("opus".into());
        m.permission_mode = Some("plan".into());
        m.system_prompt = "You are a solver.".into();
        assert_eq!(
            engine_argv(&m).expect("runnable"),
            [
                "claude",
                "--permission-mode",
                "plan",
                "--append-system-prompt",
                "You are a solver.",
                "--model",
                "opus",
            ]
        );
    }

    #[test]
    fn codex_argv_honors_model_and_sandbox() {
        let mut m = manifest("tmux:codex");
        m.model = Some("gpt-5-codex".into());
        m.extra.insert("sandbox".into(), "workspace-write".into());
        assert_eq!(
            engine_argv(&m).expect("runnable"),
            ["codex", "--sandbox", "workspace-write", "--model", "gpt-5-codex"]
        );
    }

    #[test]
    fn key_names_are_single_tmux_tokens() {
        for key in ["Enter", "Escape", "Up", "Down", "Tab", "C-c", "F5", "1"] {
            assert!(validate_key(key).is_ok(), "{key} should be valid");
        }
        // A key can never start like a flag or carry extra tmux arguments.
        for key in ["", "-l", "--", "Enter Escape", "C c", "'"] {
            assert!(
                matches!(validate_key(key), Err(Error::InvalidKey(_))),
                "{key:?} should be rejected"
            );
        }
    }

    #[test]
    fn shell_command_quotes_arguments_for_sh() {
        let cmd = shell_command(&["claude".into(), "--append-system-prompt".into(), "don't".into()]);
        assert!(cmd.contains("'claude' '--append-system-prompt' 'don'\\''t'"), "{cmd}");
        // The pane must survive a failed start so the error is readable.
        assert!(cmd.contains("read _"));
    }
}
