//! An interactive terminal per workspace: a shell started under an in-process pty in the
//! workspace's directory, observed and driven the same way agent sessions are (a screen capture
//! for the live view, keystrokes for input). Sessions are named `adi-ws-<project-prefix>-<workspace>`,
//! so they never collide with the `adi-agent-*` sessions adi-agents owns.
//!
//! The session plumbing deliberately mirrors `adi-agents/src/backends/pty` — both drive the shared
//! in-process [`adi_pty`] session manager (a real pty per session, ConPTY on Windows).

use std::path::Path;

use crate::error::{Error, Result};

/// Prefix of every pty session this module owns.
const SESSION_PREFIX: &str = "adi-ws-";

/// How many leading characters of the project id key the session name (a UUID prefix is
/// plenty unique per machine).
const PROJECT_PREFIX_LEN: usize = 8;

/// A live (or freshly started) workspace terminal: its pty session name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSession {
    /// The pty session name (`adi-ws-<project-prefix>-<workspace>`).
    pub session: String,
    /// A pty session has no external attach command; it is viewed only in the control panel. Kept
    /// (always empty) so callers that used to render an attach hint still compile.
    pub attach: String,
}

/// The pty session name for a workspace terminal. Both parts may contain `.` (valid on disk);
/// dots become dashes so the session name stays a single flat token.
#[must_use]
pub fn session_name(project_id: &str, workspace: &str) -> String {
    let prefix: String = project_id
        .chars()
        .take(PROJECT_PREFIX_LEN)
        .collect::<String>()
        .replace('.', "-");
    format!("{SESSION_PREFIX}{prefix}-{}", workspace.replace('.', "-"))
}

/// Ensure a terminal session exists for the workspace, starting a shell in `dir` if needed —
/// idempotent, so "Open terminal" on an already-open terminal just reattaches the view.
///
/// # Errors
/// [`Error::NotADir`] when the workspace directory isn't on disk, [`Error::Terminal`] when the
/// pty session can't start.
pub fn open(project_id: &str, workspace: &str, dir: &Path) -> Result<TerminalSession> {
    if !dir.is_dir() {
        return Err(Error::NotADir(dir.to_path_buf()));
    }
    let session = session_name(project_id, workspace);
    if !adi_pty::is_running(&session) {
        adi_pty::launch(&session, &shell_argv(), dir, &[])
            .map_err(|e| Error::Terminal(e.to_string()))?;
    }
    Ok(TerminalSession {
        attach: String::new(),
        session,
    })
}

/// The interactive shell to launch a workspace terminal with: `$SHELL` (falling back to `/bin/sh`)
/// on unix, `%COMSPEC%` (falling back to `cmd.exe`) elsewhere.
fn shell_argv() -> Vec<String> {
    #[cfg(unix)]
    {
        vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())]
    }
    #[cfg(not(unix))]
    {
        vec![std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())]
    }
}

/// Whether the workspace has a live terminal session.
#[must_use]
pub fn is_running(project_id: &str, workspace: &str) -> bool {
    adi_pty::is_running(&session_name(project_id, workspace))
}

/// A read-only snapshot of the terminal's visible screen. `None` when there's no session (its
/// final screen stays capturable until it is stopped, even after the shell exits).
#[must_use]
pub fn capture(project_id: &str, workspace: &str) -> Option<String> {
    adi_pty::capture(&session_name(project_id, workspace))
}

/// Send input to the terminal: `text` is typed literally (no key-name interpretation), then
/// `key` (a key name like `Enter`, `Up`, `C-c`) is pressed. Either part may be empty.
///
/// # Errors
/// [`Error::NotRunning`] when there's no live session, [`Error::InvalidKey`] for a key that
/// isn't a single key token, or [`Error::Terminal`] when the session write itself fails.
pub fn send_keys(project_id: &str, workspace: &str, text: &str, key: &str) -> Result<()> {
    if !is_running(project_id, workspace) {
        return Err(Error::NotRunning(workspace.to_string()));
    }
    adi_pty::send_keys(&session_name(project_id, workspace), text, key).map_err(|e| match e {
        adi_pty::Error::InvalidKey(k) => Error::InvalidKey(k),
        other => Error::Terminal(other.to_string()),
    })
}

/// Kill the workspace's terminal session. Returns whether a live session was found and
/// killed — idempotent, so killing an already-gone terminal is a successful `false`.
///
/// # Errors
/// [`Error::Terminal`] when the kill itself fails.
pub fn kill(project_id: &str, workspace: &str) -> Result<bool> {
    adi_pty::stop(&session_name(project_id, workspace)).map_err(|e| Error::Terminal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_are_prefixed_scoped_and_flat() {
        assert_eq!(
            session_name("3f9c77b9-f83f-4377-97fd-493db66dca70", "main"),
            "adi-ws-3f9c77b9-main"
        );
        assert_eq!(session_name("short.id", "a.b"), "adi-ws-short-id-a-b");
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
