//! User-editable updater settings — `~/.adi/mono/update/config.toml`, materialized
//! with defaults on first use so it's there to edit.

use serde::{Deserialize, Serialize};

/// Where releases are announced by default: the `manifest.json` asset of the latest
/// GitHub release. Point `manifest_url` at any static host to change channels.
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/adi-family/mono/releases/latest/download/manifest.json";

const DEFAULT_CHECK_INTERVAL_HOURS: u64 = 6;

/// The `update/config.toml` shape. Unknown fields are ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The release manifest URL the updater polls.
    pub manifest_url: String,
    /// How often the background updater agent checks, in hours (also applied at login).
    pub check_interval_hours: u64,
    /// Optional extra HTTP header sent with every request (e.g. `Authorization: Bearer …`
    /// for a private static host). Not needed for public GitHub releases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            manifest_url: DEFAULT_MANIFEST_URL.to_string(),
            check_interval_hours: DEFAULT_CHECK_INTERVAL_HOURS,
            auth_header: None,
        }
    }
}

impl Settings {
    /// Load from the module's `config.toml`, creating it from defaults on first use.
    /// A corrupt file falls back to defaults rather than blocking updates.
    #[must_use]
    pub fn load(module: &adi_config::Module) -> Self {
        module
            .file("config.toml")
            .load_or_create()
            .unwrap_or_default()
    }

    /// The check interval as launchd `StartInterval` seconds, floored at one hour so a
    /// mis-edit can't turn the updater into a tight poll loop.
    #[must_use]
    pub fn check_interval_secs(&self) -> u32 {
        let hours = self.check_interval_hours.clamp(1, 24 * 30);
        u32::try_from(hours * 3600).unwrap_or(6 * 3600)
    }
}

/// The updater's settings/state directory in the standard store (`~/.adi/mono/update`).
#[must_use]
pub fn module() -> adi_config::Module {
    adi_config::Config::open().module("update")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_the_github_latest_release() {
        let s = Settings::default();
        assert_eq!(s.manifest_url, DEFAULT_MANIFEST_URL);
        assert_eq!(s.check_interval_hours, 6);
        assert_eq!(s.auth_header, None);
    }

    #[test]
    fn interval_is_clamped_to_at_least_an_hour() {
        let s = Settings {
            check_interval_hours: 0,
            ..Settings::default()
        };
        assert_eq!(s.check_interval_secs(), 3600);
        let s = Settings {
            check_interval_hours: 6,
            ..Settings::default()
        };
        assert_eq!(s.check_interval_secs(), 6 * 3600);
    }
}
