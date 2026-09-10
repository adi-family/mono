//! The `/api/settings/shared-assets` surface: which of [`SharedAssetsMode`]'s three modes points
//! the browser's copy of the webapp bundle at the shared-assets CDN instead of this instance's
//! own `dist/`. Persisted so it survives a restart; `adi-app`'s `shared_assets` module reads
//! [`mode`] and [`base_url`] at serve time and does the actual URL rewriting — this file only
//! owns the setting itself.

use adi_config::{Config, Module};
use serde::{Deserialize, Serialize};

use crate::types::{SetSharedAssets, SharedAssetsMode, SharedAssetsState};

use super::response::{FromBody, Response, ok_json};

/// This setting's module directory (`~/.adi/mono/shared-assets`).
const MODULE: &str = "shared-assets";

/// The settings file within [`MODULE`].
const SETTINGS_FILE: &str = "settings.toml";

/// The shared-assets CDN's base URL in production — the custom domain `apps/shared-assets`'s
/// `scripts/setup-cf.sh` attaches to the R2 bucket. Overridden by `ADI_SHARED_ASSETS_BASE_URL`,
/// for testing against a bucket that isn't the production one.
pub const DEFAULT_BASE_URL: &str = "https://cdn.withadi.dev";

/// The env var that overrides [`DEFAULT_BASE_URL`].
const BASE_URL_ENV: &str = "ADI_SHARED_ASSETS_BASE_URL";

/// `shared-assets/settings.toml`'s shape.
///
/// Deserialized by hand rather than derived: a store written before this setting grew modes has
/// `enabled: bool`, not `mode`, and reading that as an unknown field — silently discarding
/// somebody's real choice to turn the CDN on — would be worse than the few match arms below.
/// `true` becomes [`SharedAssetsMode::CdnAlways`], the only mode the old boolean could mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
struct Settings {
    mode: SharedAssetsMode,
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Raw {
            mode: Option<SharedAssetsMode>,
            enabled: Option<bool>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let mode = raw.mode.unwrap_or(match raw.enabled {
            Some(true) => SharedAssetsMode::CdnAlways,
            Some(false) | None => SharedAssetsMode::LocalAlways,
        });
        Ok(Settings { mode })
    }
}

impl Settings {
    fn load(module: &Module) -> Self {
        module
            .file::<Self>(SETTINGS_FILE)
            .load_or_create()
            .unwrap_or_default()
    }

    fn save(self, module: &Module) -> std::io::Result<()> {
        module
            .file(SETTINGS_FILE)
            .save(&self)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}

/// This setting's module in the standard store.
fn module() -> Module {
    Config::open().module(MODULE)
}

/// The CDN base URL bundle references are pointed at: [`BASE_URL_ENV`] if set, else
/// [`DEFAULT_BASE_URL`].
#[must_use]
pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// The shared-assets mode currently in effect. What `adi-app` checks at serve time.
#[must_use]
pub fn mode() -> SharedAssetsMode {
    Settings::load(&module()).mode
}

/// `GET /api/settings/shared-assets`.
#[must_use]
pub fn shared_assets_state() -> Response {
    ok_json(&snapshot())
}

/// `POST /api/settings/shared-assets` — set [`SharedAssetsState::mode`] and answer with the
/// fresh state.
#[must_use]
pub fn set_shared_assets(body: &[u8]) -> Response {
    let req = require!(body, SetSharedAssets);
    let module = module();
    if let Err(e) = (Settings { mode: req.mode }).save(&module) {
        return super::response::error(500, &format!("saving shared-assets settings: {e}"));
    }
    ok_json(&snapshot())
}

fn snapshot() -> SharedAssetsState {
    SharedAssetsState {
        mode: Settings::load(&module()).mode,
        base_url: base_url(),
    }
}

impl FromBody for SetSharedAssets {
    const EXPECTED: &'static str =
        "expected { mode: \"local-always\" | \"cdn-when-remote\" | \"cdn-always\" }";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-shared-assets-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root).module(MODULE)
    }

    #[test]
    fn a_fresh_store_defaults_to_local_always() {
        let module = scratch("default");
        assert_eq!(Settings::load(&module).mode, SharedAssetsMode::LocalAlways);
        assert!(
            module.dir().join(SETTINGS_FILE).exists(),
            "the file is materialized so it can be edited by hand"
        );
    }

    #[test]
    fn every_mode_round_trips() {
        let module = scratch("round-trip");
        for mode in [
            SharedAssetsMode::LocalAlways,
            SharedAssetsMode::CdnWhenRemote,
            SharedAssetsMode::CdnAlways,
        ] {
            Settings { mode }.save(&module).expect("save");
            assert_eq!(Settings::load(&module).mode, mode);
        }
    }

    #[test]
    fn a_store_written_before_modes_existed_migrates_the_old_bool() {
        let module = scratch("migrate");
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(SETTINGS_FILE), "enabled = true").expect("write");
        assert_eq!(Settings::load(&module).mode, SharedAssetsMode::CdnAlways);

        std::fs::write(module.dir().join(SETTINGS_FILE), "enabled = false").expect("write");
        assert_eq!(Settings::load(&module).mode, SharedAssetsMode::LocalAlways);
    }

    #[test]
    fn a_corrupt_file_reads_as_local_always() {
        let module = scratch("corrupt");
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(SETTINGS_FILE), "not = [toml").expect("write");
        assert_eq!(Settings::load(&module).mode, SharedAssetsMode::LocalAlways);
    }

    #[test]
    fn base_url_falls_back_to_the_default_when_the_env_override_is_blank() {
        // SAFETY: this test only ever reads its own env var, and no other test in this process
        // touches `BASE_URL_ENV`.
        unsafe { std::env::remove_var(BASE_URL_ENV) };
        assert_eq!(base_url(), DEFAULT_BASE_URL);

        unsafe { std::env::set_var(BASE_URL_ENV, "  ") };
        assert_eq!(base_url(), DEFAULT_BASE_URL);

        unsafe { std::env::set_var(BASE_URL_ENV, "https://cdn.example.test") };
        assert_eq!(base_url(), "https://cdn.example.test");

        unsafe { std::env::remove_var(BASE_URL_ENV) };
    }
}
