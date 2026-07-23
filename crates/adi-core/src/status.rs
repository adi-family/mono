//! Reading the JSON status file `adi-dns` writes (see `crates/adi-dns/src/status.rs`).
//! The GUI-facing shape lives there as `Serialize`; here we `Deserialize` it back to
//! learn the dynamically-bound port and whether the resolver process is still alive.

use std::path::Path;

use serde::Deserialize;

use crate::proc;

/// Mirror of the fields `adi-dns` emits; extra fields are ignored so the two can evolve independently.
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonStatus {
    pub pid: i32,
    pub domain: String,
    pub bound_addr: String,
    pub port: u16,
    pub route_installed: bool,
    pub started_at_unix: u64,
    pub version: String,
}

/// Parse the status file, or `None` if it's missing or malformed.
#[must_use]
pub fn read(path: &Path) -> Option<DaemonStatus> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether `pid` is a live process.
///
/// Unix: `kill -0` (signal 0 tests existence). Windows: `tasklist` filtered to the pid — it exits
/// 0 either way, so liveness is read from the output ("No tasks are running…" means gone).
#[must_use]
pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        proc::run(&["/bin/kill", "-0", &pid.to_string()]).ok()
    }
    #[cfg(not(unix))]
    {
        let out = proc::run(&[
            "tasklist",
            "/NH",
            "/FI",
            &format!("PID eq {pid}"),
        ]);
        out.ok() && out.text.contains(&pid.to_string())
    }
}
