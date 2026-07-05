//! The JSON status file a controlling GUI reads to learn live state — the addresses
//! the proxy actually bound and how many routes it is serving. Mirrors `adi-dns`'s
//! status file so a supervisor/GUI treats every adi service the same way.

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

/// Precedence: the `ADI_HIVE_STATUS_FILE` env var, then `default` (the caller passes
/// the path beside the config, in the writable mono namespace).
pub fn resolve_path(default: PathBuf) -> PathBuf {
    if let Ok(env) = std::env::var(STATUS_FILE_ENV)
        && !env.is_empty()
    {
        return PathBuf::from(env);
    }
    default
}

pub fn write(path: &Path, status: &Status) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(status).map_err(std::io::Error::other)?;
    std::fs::write(path, json)?;
    // World-readable so a per-user GUI can read a root daemon's status file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}
