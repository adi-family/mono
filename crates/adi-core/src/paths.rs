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

/// Where per-user service definitions live.
///
/// macOS: `$HOME/Library/LaunchAgents` (launchd reads plists from here). Windows: a `tasks`
/// subdir of the mono store — Task Scheduler imports the XML by path, so it needn't live in a
/// system-watched location.
#[must_use]
pub fn launch_agents_dir() -> PathBuf {
    #[cfg(unix)]
    {
        home().join("Library").join("LaunchAgents")
    }
    #[cfg(not(unix))]
    {
        support_dir().join("tasks")
    }
}

/// Where per-user service logs live. macOS: `$HOME/Library/Logs`. Windows: a `logs` subdir of
/// the mono store.
#[must_use]
pub fn logs_dir() -> PathBuf {
    #[cfg(unix)]
    {
        home().join("Library").join("Logs")
    }
    #[cfg(not(unix))]
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

    #[cfg(unix)]
    #[test]
    fn launch_agents_and_logs_live_under_library() {
        assert!(launch_agents_dir().ends_with("Library/LaunchAgents"));
        assert!(logs_dir().ends_with("Library/Logs"));
    }

    // On Windows there is no `Library/*`: task definitions and logs live under the mono store.
    #[cfg(not(unix))]
    #[test]
    fn launch_agents_and_logs_live_under_the_store() {
        assert!(launch_agents_dir().ends_with("tasks"));
        assert!(logs_dir().ends_with("logs"));
        assert!(launch_agents_dir().starts_with(support_dir()));
    }
}
