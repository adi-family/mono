//! The JSON status file the GUI reads to learn live state — most importantly the
//! port the resolver bound, which it picks dynamically at runtime.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const STATUS_FILE_ENV: &str = "ADI_DNS_STATUS_FILE";

#[derive(Debug, Serialize)]
pub struct Status {
    pub pid: u32,
    pub domain: String,
    pub bound_addr: String,
    pub port: u16,
    pub route_installed: bool,
    pub started_at_unix: u64,
    pub version: String,
}

impl Status {
    pub fn new(domain: &str, bound: SocketAddr, route_installed: bool) -> Self {
        let started_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            pid: std::process::id(),
            domain: domain.to_string(),
            bound_addr: bound.to_string(),
            port: bound.port(),
            route_installed,
            started_at_unix,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Precedence: explicit config value, then `ADI_DNS_STATUS_FILE`, then a per-OS default.
pub fn resolve_path(configured: Option<&Path>) -> PathBuf {
    if let Some(p) = configured {
        return p.to_path_buf();
    }
    if let Ok(env) = std::env::var(STATUS_FILE_ENV)
        && !env.is_empty()
    {
        return PathBuf::from(env);
    }
    default_path()
}

fn default_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    let dir = PathBuf::from("/Library/Application Support/adi-dns");
    #[cfg(target_os = "linux")]
    let dir = PathBuf::from("/run/adi-dns");
    #[cfg(target_os = "windows")]
    let dir = {
        let base = std::env::var("PROGRAMDATA").unwrap_or_else(|_| "C:\\ProgramData".to_string());
        PathBuf::from(base).join("adi-dns")
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let dir = std::env::temp_dir();
    dir.join("status.json")
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
