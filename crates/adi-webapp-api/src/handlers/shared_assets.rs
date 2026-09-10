//! The `/api/settings/shared-assets` surface: on/off for pointing the browser's copy of the
//! webapp bundle at the shared-assets CDN instead of this instance's own `dist/`. Persisted so it
//! survives a restart; `adi-app`'s `shared_assets` module reads [`enabled`] and [`base_url`] at
//! serve time and does the actual URL rewriting — this file only owns the setting itself.

use adi_config::{Config, Module};
use serde::{Deserialize, Serialize};

use crate::types::{SetSharedAssets, SharedAssetsState};

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

/// `shared-assets/settings.toml`'s shape. Unknown fields are ignored, so an older store keeps
/// loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// Off by default — see the module doc.
    enabled: bool,
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

/// Whether the shared-assets setting is currently on. What `adi-app` checks at serve time.
#[must_use]
pub fn enabled() -> bool {
    Settings::load(&module()).enabled
}

/// `GET /api/settings/shared-assets`.
#[must_use]
pub fn shared_assets_state() -> Response {
    ok_json(&snapshot())
}

/// `POST /api/settings/shared-assets` — set [`SharedAssetsState::enabled`] and answer with the
/// fresh state.
#[must_use]
pub fn set_shared_assets(body: &[u8]) -> Response {
    let req = require!(body, SetSharedAssets);
    let module = module();
    if let Err(e) = (Settings {
        enabled: req.enabled,
    })
    .save(&module)
    {
        return super::response::error(500, &format!("saving shared-assets settings: {e}"));
    }
    ok_json(&snapshot())
}

fn snapshot() -> SharedAssetsState {
    SharedAssetsState {
        enabled: Settings::load(&module()).enabled,
        base_url: base_url(),
    }
}

impl FromBody for SetSharedAssets {
    const EXPECTED: &'static str = "expected { enabled: bool }";
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
    fn a_fresh_store_defaults_off() {
        let module = scratch("default");
        assert!(!Settings::load(&module).enabled);
        assert!(
            module.dir().join(SETTINGS_FILE).exists(),
            "the file is materialized so it can be edited by hand"
        );
    }

    #[test]
    fn toggling_round_trips() {
        let module = scratch("round-trip");
        Settings { enabled: true }.save(&module).expect("save");
        assert!(Settings::load(&module).enabled);
        Settings { enabled: false }.save(&module).expect("save");
        assert!(!Settings::load(&module).enabled);
    }

    #[test]
    fn a_corrupt_file_reads_as_off() {
        let module = scratch("corrupt");
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(SETTINGS_FILE), "not = [toml").expect("write");
        assert!(!Settings::load(&module).enabled);
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
