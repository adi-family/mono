//! A tmux terminal per workspace: an interactive shell started detached in the workspace's
//! directory, observed and driven the same way agent sessions are (capture-pane for the live
//! view, send-keys for input). Sessions are named `adi-ws-<project-prefix>-<workspace>`, so
//! they never collide with the `adi-agent-*` sessions adi-agents owns.
//!
//! The tmux plumbing deliberately mirrors `adi-agents/src/run.rs` — same exact-match
//! targeting, key validation, and launchd-safe binary resolution.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

/// Prefix of every tmux session this module owns.
const SESSION_PREFIX: &str = "adi-ws-";

/// How many leading characters of the project id key the session name (a UUID prefix is
/// plenty unique per machine, and keeps `tmux attach -t …` typeable).
const PROJECT_PREFIX_LEN: usize = 8;

/// A live (or freshly started) workspace terminal: its tmux session name and the command a
/// human runs to take it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSession {
    /// The tmux session name (`adi-ws-<project-prefix>-<workspace>`).
    pub session: String,
    /// The takeover command: `tmux attach -t <session>`.
    pub attach: String,
}

/// The tmux session name for a workspace terminal. Both parts may contain `.` (valid on
/// disk), which tmux forbids in session names, so dots become dashes.
#[must_use]
pub fn session_name(project_id: &str, workspace: &str) -> String {
    let prefix: String = project_id
        .chars()
        .take(PROJECT_PREFIX_LEN)
        .collect::<String>()
        .replace('.', "-");
    format!("{SESSION_PREFIX}{prefix}-{}", workspace.replace('.', "-"))
}

/// Ensure a terminal session exists for the workspace, starting one in `dir` if needed —
/// idempotent, so "Open terminal" on an already-open terminal just reattaches the view.
///
/// # Errors
/// [`Error::NotADir`] when the workspace directory isn't on disk, [`Error::Tmux`] when tmux
/// can't start the session.
pub fn open(project_id: &str, workspace: &str, dir: &Path) -> Result<TerminalSession> {
    if !dir.is_dir() {
        return Err(Error::NotADir(dir.to_path_buf()));
    }
    let session = session_name(project_id, workspace);
    if !session_exists(&session) {
        let dir_str = dir.to_string_lossy();
        run_tmux(&["new-session", "-d", "-s", &session, "-c", &dir_str])?;
    }
    Ok(TerminalSession {
        attach: format!("tmux attach -t {session}"),
        session,
    })
}

/// Whether the workspace has a live terminal session.
#[must_use]
pub fn is_running(project_id: &str, workspace: &str) -> bool {
    session_exists(&session_name(project_id, workspace))
}

/// A read-only snapshot of the terminal's visible pane — the text `tmux attach` would show.
/// `None` when there's no live session (or tmux isn't available).
#[must_use]
pub fn capture(project_id: &str, workspace: &str) -> Option<String> {
    let session = session_name(project_id, workspace);
    // The trailing `:` makes this a valid target-pane (exact-match session, default window
    // and pane). A bare `=name` is only a target-session — capture-pane rejects it.
    let out = Command::new(tmux_bin())
        .args(["capture-pane", "-t", &format!("={session}:"), "-p"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Send input to the terminal: `text` is typed literally (no key-name interpretation), then
/// `key` (a tmux key name like `Enter`, `Up`, `C-c`) is pressed. Either part may be empty.
///
/// # Errors
/// [`Error::NotRunning`] when there's no live session, [`Error::InvalidKey`] for a key that
/// isn't a single tmux key token, or [`Error::Tmux`] when tmux itself fails.
pub fn send_keys(project_id: &str, workspace: &str, text: &str, key: &str) -> Result<()> {
    let session = session_name(project_id, workspace);
    if !session_exists(&session) {
        return Err(Error::NotRunning(workspace.to_string()));
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

/// Kill the workspace's terminal session. Returns whether a live session was found and
/// killed — idempotent, so killing an already-gone terminal is a successful `false`.
///
/// # Errors
/// [`Error::Tmux`] when the kill itself fails on an existing session.
pub fn kill(project_id: &str, workspace: &str) -> Result<bool> {
    let session = session_name(project_id, workspace);
    if !session_exists(&session) {
        return Ok(false);
    }
    run_tmux(&["kill-session", "-t", &format!("={session}")])?;
    Ok(true)
}

/// A key must be one tmux key token (`Enter`, `Escape`, `Up`, `C-c`, `F5`, …): alphanumeric
/// with inner dashes. The leading character is kept strict so a key can never parse as a
/// tmux flag.
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

/// Whether a tmux session with exactly this name exists (`=` pins tmux to an exact match
/// instead of its default prefix matching).
fn session_exists(session: &str) -> bool {
    Command::new(tmux_bin())
        .args(["has-session", "-t", &format!("={session}")])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The tmux binary: the first well-known absolute location that exists, falling back to a
/// PATH lookup. The absolute candidates matter because the app daemon runs under launchd,
/// whose PATH misses Homebrew.
fn tmux_bin() -> &'static str {
    ["/opt/homebrew/bin/tmux", "/usr/local/bin/tmux", "/usr/bin/tmux"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("tmux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_are_prefixed_scoped_and_tmux_safe() {
        assert_eq!(
            session_name("3f9c77b9-f83f-4377-97fd-493db66dca70", "main"),
            "adi-ws-3f9c77b9-main"
        );
        // tmux forbids '.' in session names; it must be mangled, not passed through.
        assert_eq!(session_name("short.id", "a.b"), "adi-ws-short-id-a-b");
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
    fn open_requires_the_directory_on_disk() {
        let missing = std::env::temp_dir().join("adi-hooks-term-missing-dir");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(matches!(
            open("someid", "main", &missing),
            Err(Error::NotADir(_))
        ));
    }
}
