//! Host-side writer for `<workforce_dir>/<employee>/sdk_log.jsonl` — the
//! same file the WASM `log()` import persists to. Lets Rust-side
//! triggers and tools push lines into the Logs UI alongside SDK calls.
//!
//! Failures are deliberately swallowed. Observability must not break
//! the loop that's producing events.

use std::io::Write;
use std::path::Path;

pub fn record(workforce_dir: &Path, employee: &str, level: &str, message: &str) {
    // Intentionally no eprintln here — callers already decide their
    // own stderr formatting (some prefer the display-name prefix).
    // This helper only handles the persistent JSONL side.
    let emp_dir = workforce_dir.join(employee);
    if std::fs::create_dir_all(&emp_dir).is_err() {
        return;
    }
    let path = emp_dir.join("sdk_log.jsonl");
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let line = serde_json::json!({
        "ts": ts_ms,
        "level": level,
        "message": message,
    });
    let _ = writeln!(f, "{}", line);
}

pub fn info(workforce_dir: &Path, employee: &str, message: &str) {
    record(workforce_dir, employee, "info", message);
}

pub fn warn(workforce_dir: &Path, employee: &str, message: &str) {
    record(workforce_dir, employee, "warn", message);
}
