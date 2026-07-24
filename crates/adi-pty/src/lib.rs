//! A cross-platform interactive terminal-session manager: the in-process replacement for the
//! external tmux server that agent runs and workspace terminals used to be driven through. Each
//! session spawns a command under a real pseudo-terminal (ConPTY on Windows, a unix pty elsewhere,
//! via `portable-pty`); a reader thread feeds the child's output into a `vt100` screen model, and
//! the visible screen is snapshotted for the live view. Input is written straight to the pty.
//!
//! Sessions live in a process-global registry keyed by name, so — unlike tmux's own server — they
//! last only as long as this process and are visible only to it. A session whose child exits is
//! kept (marked not-running) so its final screen stays capturable until it is stopped or relaunched.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const ROWS: u16 = 40;
const COLS: u16 = 120;

#[derive(Debug)]
pub enum Error {
    InvalidKey(String),
    Launch(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKey(k) => write!(f, "invalid key token {k:?}"),
            Self::Launch(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for Error {}

struct Session {
    _master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    alive: Arc<AtomicBool>,
}

static SESSIONS: LazyLock<Mutex<BTreeMap<String, Session>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Launch `argv` under a fresh pty as session `name`, in `cwd`, with `env` overlaid on the
/// inherited environment. Caller must ensure no live session of that name exists (check
/// [`is_running`]); a dead session of that name is replaced.
pub fn launch(name: &str, argv: &[String], cwd: &Path, env: &[(String, String)]) -> Result<(), Error> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| Error::Launch("empty command".to_string()))?;
    let pair = native_pty_system()
        .openpty(PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| Error::Launch(format!("open pty: {e}")))?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.cwd(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| Error::Launch(format!("spawn: {e}")))?;
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| Error::Launch(format!("reader: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| Error::Launch(format!("writer: {e}")))?;
    let killer = child.clone_killer();
    let parser = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
    let alive = Arc::new(AtomicBool::new(true));
    {
        let parser = Arc::clone(&parser);
        let alive = Arc::clone(&alive);
        let mut reader = reader;
        let mut child = child;
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut p) = parser.lock() {
                            p.process(&buf[..n]);
                        }
                    }
                }
            }
            let _ = child.wait();
            alive.store(false, Ordering::Relaxed);
        });
    }
    SESSIONS.lock().unwrap().insert(
        name.to_string(),
        Session { _master: pair.master, writer, killer, parser, alive },
    );
    Ok(())
}

#[must_use]
pub fn is_running(name: &str) -> bool {
    SESSIONS.lock().unwrap().get(name).is_some_and(|s| s.alive.load(Ordering::Relaxed))
}

#[must_use]
pub fn running(prefix: &str) -> BTreeSet<String> {
    SESSIONS.lock().unwrap().iter()
        .filter(|(n, s)| n.starts_with(prefix) && s.alive.load(Ordering::Relaxed))
        .map(|(n, _)| n.clone()).collect()
}

#[must_use]
pub fn capture(name: &str) -> Option<String> {
    let reg = SESSIONS.lock().unwrap();
    let session = reg.get(name)?;
    let parser = session.parser.lock().ok()?;
    Some(parser.screen().contents().trim_end().to_string())
}

pub fn send_keys(name: &str, text: &str, key: &str) -> Result<(), Error> {
    let key_bytes = if key.is_empty() { Vec::new() } else { key_to_bytes(key)? };
    let mut reg = SESSIONS.lock().unwrap();
    let Some(session) = reg.get_mut(name) else { return Ok(()); };
    if !text.is_empty() {
        let _ = session.writer.write_all(text.as_bytes());
    }
    if !key_bytes.is_empty() {
        let _ = session.writer.write_all(&key_bytes);
    }
    let _ = session.writer.flush();
    Ok(())
}

pub fn stop(name: &str) -> Result<bool, Error> {
    let mut session = match SESSIONS.lock().unwrap().remove(name) {
        Some(s) => s,
        None => return Ok(false),
    };
    let was_live = session.alive.load(Ordering::Relaxed);
    let _ = session.killer.kill();
    Ok(was_live)
}

fn key_to_bytes(key: &str) -> Result<Vec<u8>, Error> {
    if let Some(rest) = key.strip_prefix("C-") {
        let mut chars = rest.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Ok(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
            }
        }
    }
    let seq: &[u8] = match key {
        "Enter" => b"\r",
        "Escape" => b"\x1b",
        "Tab" => b"\t",
        "Space" => b" ",
        "BSpace" | "Backspace" => b"\x7f",
        "Up" => b"\x1b[A",
        "Down" => b"\x1b[B",
        "Right" => b"\x1b[C",
        "Left" => b"\x1b[D",
        "Home" => b"\x1b[H",
        "End" => b"\x1b[F",
        "PageUp" => b"\x1b[5~",
        "PageDown" => b"\x1b[6~",
        "Delete" | "DC" => b"\x1b[3~",
        "Insert" | "IC" => b"\x1b[2~",
        "F1" => b"\x1bOP",
        "F2" => b"\x1bOQ",
        "F3" => b"\x1bOR",
        "F4" => b"\x1bOS",
        "F5" => b"\x1b[15~",
        "F6" => b"\x1b[17~",
        "F7" => b"\x1b[18~",
        "F8" => b"\x1b[19~",
        "F9" => b"\x1b[20~",
        "F10" => b"\x1b[21~",
        "F11" => b"\x1b[23~",
        "F12" => b"\x1b[24~",
        other => {
            let mut chars = other.chars();
            if let (Some(_), None) = (chars.next(), chars.next()) {
                return Ok(other.as_bytes().to_vec());
            }
            return Err(Error::InvalidKey(key.to_string()));
        }
    };
    Ok(seq.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_tokens_encode_to_expected_bytes() {
        assert_eq!(key_to_bytes("Enter").unwrap(), b"\r");
        assert_eq!(key_to_bytes("Up").unwrap(), b"\x1b[A");
        assert_eq!(key_to_bytes("C-c").unwrap(), vec![0x03]);
        assert_eq!(key_to_bytes("a").unwrap(), b"a");
        assert!(matches!(key_to_bytes("Enter Escape"), Err(Error::InvalidKey(_))));
    }
    #[test]
    fn absent_session_is_inert() {
        assert!(!is_running("adi-pty-test-absent"));
        assert!(capture("adi-pty-test-absent").is_none());
    }
}
