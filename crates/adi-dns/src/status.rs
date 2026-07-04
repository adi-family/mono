//! The status file — a tiny JSON heartbeat the GUI/menu-bar reads to show live
//! state (running? which port did it bind? is the OS route installed?).
//!
//! The resolver picks its port dynamically (preferred, else a fallback), so the
//! controlling app can't know it up front — it learns it from here. Written once
//! the listener is bound and removed on clean shutdown. Writing is **best-effort**:
//! a failure (e.g. no permission to the state dir) is never fatal to serving.
//!
//! The file is made world-readable so a per-user GUI can read the status a
//! **root** `LaunchDaemon` writes.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Environment variable that overrides the status-file path (set by the launchd
/// plist / systemd unit so the daemon and its GUI agree on one location).
const STATUS_FILE_ENV: &str = "ADI_DNS_STATUS_FILE";

/// A snapshot of the running resolver, serialized to the status file.
#[derive(Debug, Serialize)]
pub struct Status {
    /// PID of the running resolver process.
    pub pid: u32,
    /// The TLD this resolver owns (e.g. `adi`).
    pub domain: String,
    /// The address:port the listener actually bound.
    pub bound_addr: String,
    /// The port it actually bound (convenience; also present in `bound_addr`).
    pub port: u16,
    /// Whether the OS route for `.domain` was installed this run.
    pub route_installed: bool,
    /// Unix time (seconds) the resolver became ready.
    pub started_at_unix: u64,
    /// Crate version of the running binary.
    pub version: String,
}

impl Status {
    /// Capture the current process state for a resolver bound at `bound`.
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

/// Resolve the status-file path: explicit config value first, then the
/// [`STATUS_FILE_ENV`] override, then a per-OS default.
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

/// The per-OS default location, used when nothing overrides it.
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

/// Write the status file, creating its parent directory if needed. Best-effort:
/// the caller logs and keeps serving on error.
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

/// Remove the status file on shutdown. Best-effort; a missing file is not an error.
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}
