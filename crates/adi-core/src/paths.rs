//! Where the mono-app keeps its runtime files. The store directory itself
//! (`$HOME/<ADI_DIR>/mono`, `ADI_DIR` defaulting to `.adi`) is owned by the shared
//! [`adi_config`] store; this module only adds the macOS `Library/*` locations that
//! sit outside it, mirroring the Swift `AppPaths`.

use std::path::PathBuf;

/// `$HOME`, or `/` if unset (matching `NSHomeDirectory` fallbacks closely enough).
fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The `ADI_DIR` value, trimmed; empty/unset falls back to `.adi`.
#[must_use]
pub fn dir_name() -> String {
    adi_config::dir_name()
}

/// `$HOME/<ADI_DIR>/mono` — the mono-app store directory.
#[must_use]
pub fn support_dir() -> PathBuf {
    adi_config::dir()
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
    fn support_dir_is_under_the_mono_namespace() {
        assert!(support_dir().ends_with("mono"));
    }

    #[test]
    fn launch_agents_and_logs_live_under_library() {
        assert!(launch_agents_dir().ends_with("Library/LaunchAgents"));
        assert!(logs_dir().ends_with("Library/Logs"));
    }
}
