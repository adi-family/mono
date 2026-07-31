//! Where the mono-app keeps its runtime files. The store directory itself
//! (`$HOME/<ADI_DIR>/mono`, `ADI_DIR` defaulting to `.adi`) is owned by the shared
//! [`adi_config`] store; this module only adds the macOS `Library/*` locations that
//! sit outside it, mirroring the Swift `AppPaths`.

use std::path::PathBuf;

/// The user's home directory (cross-platform, resolved by [`adi_config::home`]).
#[cfg(unix)]
fn home() -> PathBuf {
    adi_config::home()
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

/// `$XDG_CONFIG_HOME/systemd/user`, or `$HOME/.config/systemd/user` — the only directory a
/// `systemd --user` manager reads unit files from, so this one is not a free choice.
#[cfg(target_os = "linux")]
fn systemd_user_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home().join(".config"), PathBuf::from)
        .join("systemd")
        .join("user")
}

/// Where per-user service definitions live.
///
/// macOS: `$HOME/Library/LaunchAgents` (launchd reads plists from here). Linux:
/// `~/.config/systemd/user` (likewise fixed by systemd). Windows: a `tasks` subdir of the mono
/// store — Task Scheduler imports the XML by path, so it needn't live in a system-watched location.
#[must_use]
pub fn launch_agents_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home().join("Library").join("LaunchAgents")
    }
    #[cfg(target_os = "linux")]
    {
        systemd_user_dir()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        support_dir().join("tasks")
    }
}

/// Where per-user service logs live. macOS: `$HOME/Library/Logs`. Linux and Windows: a `logs`
/// subdir of the mono store.
///
/// Linux has no per-user equivalent of `Library/Logs`, and the obvious alternative — the journal —
/// is not one: the panel, `adi logs`, and every service's own writer all address a *file path*.
/// The unit therefore redirects to this same file (`StandardOutput=append:`), which keeps one
/// answer to "where are the logs" on all three platforms.
#[must_use]
pub fn logs_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home().join("Library").join("Logs")
    }
    #[cfg(not(target_os = "macos"))]
    {
        support_dir().join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_dir_is_under_the_mono_namespace() {
        assert!(support_dir().ends_with("mono"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agents_and_logs_live_under_library() {
        assert!(launch_agents_dir().ends_with("Library/LaunchAgents"));
        assert!(logs_dir().ends_with("Library/Logs"));
    }

    // Linux: units go where systemd looks for them, logs into the store.
    #[cfg(target_os = "linux")]
    #[test]
    fn units_live_where_systemd_reads_them() {
        assert!(launch_agents_dir().ends_with(".config/systemd/user"));
        assert!(logs_dir().ends_with("logs"));
        assert!(logs_dir().starts_with(support_dir()));
    }

    // On Windows there is no `Library/*`: task definitions and logs live under the mono store.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn launch_agents_and_logs_live_under_the_store() {
        assert!(launch_agents_dir().ends_with("tasks"));
        assert!(logs_dir().ends_with("logs"));
        assert!(launch_agents_dir().starts_with(support_dir()));
    }
}
