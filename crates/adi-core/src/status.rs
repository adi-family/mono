//! Reading the JSON status file `adi-dns` writes (see `crates/adi-dns/src/status.rs`).
//! The GUI-facing shape lives there as `Serialize`; here we `Deserialize` it back to
//! learn the dynamically-bound port and whether the resolver process is still alive.

use std::path::Path;

use serde::Deserialize;

use crate::proc;

/// Mirror of the fields `adi-dns` emits. Extra fields are ignored so the two can
/// evolve independently.
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

/// Whether `pid` is a live process, probed with `kill -0` (signal 0 tests existence
/// without delivering a signal). The resolver runs as the same uid, so a bare exit
/// code is a reliable liveness check here.
#[must_use]
pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    proc::run(&["/bin/kill", "-0", &pid.to_string()]).ok()
}
