//! The JSON status file a controlling GUI reads to learn live state — the addresses the
//! proxy bound and how many routes it serves. Mirrors `adi-dns`'s status file.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const STATUS_FILE_ENV: &str = "ADI_HIVE_STATUS_FILE";

#[derive(Debug, Serialize)]
pub struct Status {
    pub pid: u32,
    pub bound_addrs: Vec<String>,
    pub route_count: usize,
    pub started_at_unix: u64,
    pub version: String,
}

impl Status {
    /// Snapshot the live state a controlling GUI reads: what bound, and how much is routed.
    #[must_use]
    pub fn new(bound_addrs: Vec<String>, route_count: usize) -> Self {
        let started_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            pid: std::process::id(),
            bound_addrs,
            route_count,
            started_at_unix,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Precedence: the `ADI_HIVE_STATUS_FILE` env var, then `default`.
#[must_use]
pub fn resolve_path(default: PathBuf) -> PathBuf {
    if let Ok(env) = std::env::var(STATUS_FILE_ENV)
        && !env.is_empty()
    {
        return PathBuf::from(env);
    }
    default
}

/// Write the status file, world-readable so a controlling GUI can read a root daemon's.
///
/// # Errors
/// Fails if the status can't be encoded, its directory can't be created, or the file can't be
/// written. Callers log it and keep serving; the status file is a report on the daemon, never a
/// thing the daemon needs.
pub fn write(path: &Path, status: &Status) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(status).map_err(std::io::Error::other)?;
    adi_osext::write_status_file(path, &json)
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}
