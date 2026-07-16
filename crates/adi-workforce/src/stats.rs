//! Lightweight per-event stats.
//!
//! Each call to [`record`] appends the current unix-millis timestamp as
//! a new line to a plain file under `<workforce_dir>/<employee>/<kind>/<name>`.
//!
//! The debug UI can then derive:
//!   - **count** from the line count
//!   - **last fire** from the last line
//!   - **any rate / histogram** from the full timestamp list
//!
//! Why append-only timestamps instead of a counter:
//!   - No read-modify-write, so no races to worry about.
//!   - Writes <4 KB with `O_APPEND` are atomic on Unix — two concurrent
//!     writers interleave as whole lines, never byte-mashed.
//!   - All derived stats (count, last, rates) are computable from the
//!     same file, so the storage format doesn't constrain the UI.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Append a single event timestamp to `<workforce_dir>/<employee>/<kind>/<name>`.
///
/// `kind` is one of `"loop"`, `"tool"`, `"trigger"`. Unknown kinds are
/// permitted but by convention the debug UI only renders these three.
///
/// Failures (disk full, permission denied, …) are silently ignored —
/// stats are best-effort telemetry, not critical state. They must not
/// break the hot loop-runner path.
pub fn record(workforce_dir: &Path, employee: &str, kind: &str, name: &str) {
    // Sanitize to prevent path traversal if name ever contains "/" or "..".
    // Names come from trait methods and Lua config strings — should be
    // well-formed already but be defensive.
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return;
    }

    let dir = workforce_dir.join(employee).join(kind);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(name);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        // Build `"ts\n"` first, then issue ONE write_all. `writeln!`
        // can emit two write() syscalls (one for the body, one for the
        // newline) and under concurrency the newline drifts off its
        // line — producing `17758841581775884159` style concatenations.
        // A single write_all is atomic under O_APPEND for sizes under
        // PIPE_BUF (4 KB).
        let line = format!("{ts}\n");
        let _ = f.write_all(line.as_bytes());
    }
}

/// Max characters to keep for args/result previews in the JSONL log.
/// Tool outputs can be hundreds of KB; we only want a fingerprint.
const PREVIEW_CAP: usize = 800;

fn truncate_preview(s: &str) -> String {
    // Char-safe truncation — string slicing on bytes panics on multi-byte chars.
    if s.chars().count() <= PREVIEW_CAP {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(PREVIEW_CAP)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}…", &s[..end])
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Append one structured JSONL line to `<workforce_dir>/<employee>/tool_calls.jsonl`
/// with per-call details: tool name, previewed args/result, error flag, duration.
///
/// This is the counterpart to [`record`]: `record` gives you counts and
/// firing timestamps per tool; `record_tool_call_detail` gives you the
/// full audit trail (what args, what result, how long, error or not).
///
/// Previews are capped at [`PREVIEW_CAP`] chars so one bloated result
/// can't balloon the file. Failures are silently ignored.
#[allow(clippy::too_many_arguments)]
pub fn record_tool_call_detail(
    workforce_dir: &Path,
    employee: &str,
    loop_id: &str,
    run_id: &str,
    turn: usize,
    tool: &str,
    args: &str,
    result: &str,
    is_error: bool,
    duration_ms: u128,
) {
    if employee.is_empty() || employee.contains('/') || employee.contains("..") {
        return;
    }

    let emp_dir = workforce_dir.join(employee);
    if std::fs::create_dir_all(&emp_dir).is_err() {
        return;
    }
    let path = emp_dir.join("tool_calls.jsonl");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let line = format!(
        "{{\"ts\":{ts},\"loop_id\":\"{}\",\"run_id\":\"{}\",\"turn\":{turn},\"tool\":\"{}\",\"args\":\"{}\",\"result\":\"{}\",\"is_error\":{is_error},\"duration_ms\":{duration_ms}}}\n",
        json_escape(loop_id),
        json_escape(run_id),
        json_escape(tool),
        json_escape(&truncate_preview(args)),
        json_escape(&truncate_preview(result)),
    );

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Trigger liveness heartbeat.
///
/// Each trigger's polling loop calls [`write_heartbeat`] once per
/// iteration. A daemon-side watchdog then reads [`read_heartbeat`] on a
/// schedule and treats stale entries as a stalled watcher (usually a
/// sqlite lock or filesystem call that never returns). This is the only
/// observable way to distinguish "watcher is idle" from "watcher is dead"
/// — `.status=running` stays true either way.
///
/// Storage: one file per (employee, trigger) at
/// `<workforce_dir>/<employee>/trigger_heartbeat/<name>` holding a single
/// unix-millis timestamp (overwritten, not appended — we only care about
/// the latest). Truncate-write is atomic enough on unix for single-line
/// files under `PIPE_BUF`.
pub fn heartbeat_path(workforce_dir: &Path, employee: &str, name: &str) -> PathBuf {
    workforce_dir
        .join(employee)
        .join("trigger_heartbeat")
        .join(name)
}

pub fn write_heartbeat(workforce_dir: &Path, employee: &str, name: &str) {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return;
    }
    let path = heartbeat_path(workforce_dir, employee, name);
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // Write-then-rename would be atomic across readers, but a single
    // write_all of a sub-PIPE_BUF string is good enough for a watchdog
    // that polls every minute — torn reads only delay detection one cycle.
    let _ = std::fs::write(&path, format!("{ts}\n"));
}

/// Returns the heartbeat timestamp (unix millis) or 0 if missing/unreadable.
pub fn read_heartbeat(workforce_dir: &Path, employee: &str, name: &str) -> u128 {
    let path = heartbeat_path(workforce_dir, employee, name);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u128>().ok())
        .unwrap_or(0)
}
