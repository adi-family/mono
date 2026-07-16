//! The updater's persisted record of its last check/install — `update/state.json`.
//! What `adi-mono update status` prints and the GUI service row summarizes.

use serde::{Deserialize, Serialize};

/// Last-known updater state. All fields optional so the shape can grow.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Unix time of the last completed check (successful or not).
    pub last_check_unix: Option<u64>,
    /// The installed app version as of the last check/install.
    pub installed_version: Option<String>,
    /// The newest published version seen.
    pub latest_version: Option<String>,
    /// `up-to-date` | `update-available` | `installed` | `error`.
    pub last_outcome: Option<String>,
    /// The error message when `last_outcome == "error"`.
    pub last_error: Option<String>,
    /// Unix time of the last successful install.
    pub last_install_unix: Option<u64>,
}

const FILE: &str = "state.json";

impl State {
    /// Read from the module dir; missing or corrupt state is just default.
    #[must_use]
    pub fn load(module: &adi_config::Module) -> Self {
        module
            .read_raw(FILE)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Persist atomically; best-effort (state is advisory, never worth failing an update over).
    pub fn save(&self, module: &adi_config::Module) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = module.write_raw(FILE, &bytes);
        }
    }
}

/// Seconds since the Unix epoch (0 if the clock is before it).
#[must_use]
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "adi-update-state-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    #[test]
    fn round_trips_through_the_module_dir() {
        let dir = scratch("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let module = adi_config::Config::with_root(&dir).module("update");

        assert!(State::load(&module).last_check_unix.is_none());

        let state = State {
            last_check_unix: Some(123),
            installed_version: Some("0.1.0".to_string()),
            latest_version: Some("0.2.0".to_string()),
            last_outcome: Some("update-available".to_string()),
            ..State::default()
        };
        state.save(&module);

        let loaded = State::load(&module);
        assert_eq!(loaded.last_check_unix, Some(123));
        assert_eq!(loaded.latest_version.as_deref(), Some("0.2.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_state_reads_as_default() {
        let dir = scratch("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        let module = adi_config::Config::with_root(&dir).module("update");
        module.write_raw(FILE, b"{ not json").expect("write");
        assert!(State::load(&module).last_check_unix.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
