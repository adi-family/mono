//! Where the mono-app keeps its runtime files, mirroring the Swift `AppPaths`.
//!
//! Base directory: `$HOME/<ADI_DIR>/mono`, where `<ADI_DIR>` is the `ADI_DIR` env
//! var (default `.adi`, the adi platform home). The `mono` subdir isolates this
//! app's files from the platform's own (`hive`/`cocoon`/`workforce`). A
//! login-launched `LaunchAgent` only sees env vars set in the launchd session
//! (`launchctl setenv ADI_DIR …`), not shell exports.

use std::path::PathBuf;

const DEFAULT_DIR: &str = ".adi";
const MONO_SUBDIR: &str = "mono";

/// `$HOME`, or `/` if unset (matching `NSHomeDirectory` fallbacks closely enough).
fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The `ADI_DIR` value, trimmed; empty/unset falls back to `.adi`. Split out as a
/// pure function so the precedence is unit-testable without touching the env.
fn resolve_dir_name(env: Option<&str>) -> String {
    match env {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => DEFAULT_DIR.to_string(),
    }
}

#[must_use]
pub fn dir_name() -> String {
    resolve_dir_name(std::env::var("ADI_DIR").ok().as_deref())
}

/// `$HOME/<ADI_DIR>/mono` — the mono-app namespace root.
#[must_use]
pub fn support_dir() -> PathBuf {
    home().join(dir_name()).join(MONO_SUBDIR)
}

/// `$HOME/Library/LaunchAgents` — where per-user `LaunchAgent` plists live.
#[must_use]
pub fn launch_agents_dir() -> PathBuf {
    home().join("Library").join("LaunchAgents")
}

/// `$HOME/Library/Logs` — where per-user service logs live.
#[must_use]
pub fn logs_dir() -> PathBuf {
    home().join("Library").join("Logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_name_prefers_env_when_present() {
        assert_eq!(resolve_dir_name(Some(".custom")), ".custom");
        assert_eq!(resolve_dir_name(Some("  spaced  ")), "spaced");
    }

    #[test]
    fn dir_name_falls_back_to_default() {
        assert_eq!(resolve_dir_name(None), DEFAULT_DIR);
        assert_eq!(resolve_dir_name(Some("   ")), DEFAULT_DIR);
        assert_eq!(resolve_dir_name(Some("")), DEFAULT_DIR);
    }
}
