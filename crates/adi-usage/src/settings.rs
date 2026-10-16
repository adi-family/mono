//! The usage-events on/off switch: `~/.adi/mono/analytics/settings.toml`, plus the two env
//! overrides every reader folds in ahead of it.
//!
//! Everything in this crate, the panel's setting and `adi-mono analytics` all read [`effective`]
//! rather than [`Settings`] directly, so the override order lives in one place.

use adi_config::Module;
use serde::{Deserialize, Serialize};

/// What a fresh install does until the operator chooses (ADI-MONO-157 — the brief this crate
/// implements was approved before that choice was made). Flip this one constant when they do;
/// every reader here goes through [`effective`], so nothing else needs to change.
pub const DEFAULT_ENABLED: bool = false;

/// Set to `0`/`false`/`off`/`no` to force usage events off regardless of [`Settings::enabled`],
/// or to anything else non-empty to force them on. Checked ahead of the stored setting but behind
/// [`DO_NOT_TRACK_ENV`] — the one override this crate trusts more than an operator's own flag,
/// since it is the more explicit "I asked for this everywhere" signal.
pub const ENV_VAR: &str = "ADI_ANALYTICS";

/// The cross-tool convention (<https://consoledonottrack.com>). Any non-empty value other than
/// `0`/`false`/`off`/`no` forces usage events off, full stop — it is not a two-way switch here,
/// only ever an opt-out.
pub const DO_NOT_TRACK_ENV: &str = "DO_NOT_TRACK";

const SETTINGS_FILE: &str = "settings.toml";

/// `analytics/settings.toml`'s whole shape — one field, on purpose (§7 of the plan: "a toggle
/// under Settings, and the same toggle on the first-run screen").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
        }
    }
}

impl Settings {
    /// Load from disk, materializing [`DEFAULT_ENABLED`] on first read so there is a file for the
    /// operator to edit by hand if they want to.
    #[must_use]
    pub fn load(module: &Module) -> Self {
        module
            .file(SETTINGS_FILE)
            .load_or_create()
            .unwrap_or_default()
    }

    /// # Errors
    /// [`adi_config::Error::Io`] on a write failure.
    pub fn save(self, module: &Module) -> adi_config::Result<()> {
        module.file(SETTINGS_FILE).save(&self)
    }
}

/// Persist a fresh choice — what the panel's toggle and `adi-mono analytics enable`/`disable`
/// call.
///
/// # Errors
/// [`adi_config::Error::Io`] on a write failure.
pub fn set_enabled(module: &Module, enabled: bool) -> adi_config::Result<()> {
    Settings { enabled }.save(module)
}

/// Which env var is overriding the stored setting right now, and to what — `None` when neither is
/// set, so [`Settings::enabled`] decides on its own.
#[must_use]
pub fn env_override() -> Option<(&'static str, bool)> {
    if env_flag(DO_NOT_TRACK_ENV) == Some(true) {
        return Some((DO_NOT_TRACK_ENV, false));
    }
    env_flag(ENV_VAR).map(|v| (ENV_VAR, v))
}

/// Whether usage events are actually on right now: an env override if either is set, else the
/// persisted switch. The one answer every caller in this crate (and built on it) reads — never
/// [`Settings::enabled`] on its own.
#[must_use]
pub fn effective(module: &Module) -> bool {
    env_override().map_or_else(|| Settings::load(module).enabled, |(_, v)| v)
}

/// `name`'s value as a tri-state flag: unset or blank is "not asked"; `0`/`false`/`off`/`no`
/// (case-insensitive) is explicitly off; anything else non-empty is on.
fn env_flag(name: &str) -> Option<bool> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(!matches!(
        raw.to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    ))
}

/// Guards [`ENV_VAR`]/[`DO_NOT_TRACK_ENV`] across every test in this crate that touches either —
/// not just the ones in this module. Both are process-global state, `cargo test` runs a crate's
/// tests on multiple threads by default, and `dispatch`'s tests read [`effective`] (and therefore
/// both vars) without ever setting them themselves — without this lock, a `with_env` call here
/// could flip either var out from under a concurrently-running `dispatch` test.
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-usage-settings-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root).module("analytics")
    }

    // SAFETY: `ENV_TEST_LOCK` serializes every test (in this module and in `dispatch`) that
    // reads or writes either env var, and this closure clears both before returning.
    fn with_env(pairs: &[(&str, Option<&str>)], f: impl FnOnce()) {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for (name, value) in pairs {
            match value {
                Some(v) => unsafe { std::env::set_var(name, v) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        f();
        for (name, _) in pairs {
            unsafe { std::env::remove_var(name) };
        }
    }

    #[test]
    fn a_fresh_store_defaults_to_off() {
        let module = scratch("default");
        // DEFAULT_ENABLED is `false` on this branch (ADI-MONO-157: the operator has not chosen
        // yet) — this just checks that a fresh store actually reads that constant back.
        assert_eq!(Settings::load(&module).enabled, DEFAULT_ENABLED);
        assert!(module.dir().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn the_stored_setting_round_trips() {
        let module = scratch("roundtrip");
        with_env(&[(ENV_VAR, None), (DO_NOT_TRACK_ENV, None)], || {
            set_enabled(&module, true).expect("save");
            assert!(effective(&module));
            set_enabled(&module, false).expect("save");
            assert!(!effective(&module));
        });
    }

    #[test]
    fn adi_analytics_overrides_the_stored_setting_either_way() {
        let module = scratch("override");
        set_enabled(&module, true).expect("save");
        with_env(&[(ENV_VAR, Some("0")), (DO_NOT_TRACK_ENV, None)], || {
            assert!(!effective(&module), "ADI_ANALYTICS=0 forces it off");
            assert_eq!(env_override(), Some((ENV_VAR, false)));
        });

        set_enabled(&module, false).expect("save");
        with_env(&[(ENV_VAR, Some("1")), (DO_NOT_TRACK_ENV, None)], || {
            assert!(effective(&module), "ADI_ANALYTICS=1 forces it on");
        });
    }

    #[test]
    fn do_not_track_wins_over_an_explicit_adi_analytics_1() {
        let module = scratch("dnt-wins");
        set_enabled(&module, true).expect("save");
        with_env(
            &[(ENV_VAR, Some("1")), (DO_NOT_TRACK_ENV, Some("1"))],
            || {
                assert!(!effective(&module), "DO_NOT_TRACK=1 is the stronger signal");
            },
        );
    }

    #[test]
    fn a_blank_env_var_is_not_asked() {
        let module = scratch("blank");
        set_enabled(&module, true).expect("save");
        with_env(&[(ENV_VAR, Some("  ")), (DO_NOT_TRACK_ENV, None)], || {
            assert!(effective(&module), "a blank value leaves the stored setting alone");
        });
    }
}
