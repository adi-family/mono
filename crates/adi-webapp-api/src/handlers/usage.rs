//! `/api/usage-events` — the usage-events on/off switch and install id (ADI-MONO-157), for the
//! Settings page's toggle and the onboarding welcome screen's "Getting started" link alike.
//!
//! Named apart from `/api/analytics` (`handlers::agents`' per-agent run stats, a pre-existing and
//! unrelated page) deliberately — see `UsageEventsState`'s own doc comment.
//!
//! Everything here is a thin read/write over `adi_usage::settings`/`adi_usage::state` — this
//! crate owns no state of its own, so the panel and `adi-mono analytics` are reading and writing
//! exactly the same files `adi-usage`'s own dispatch loop does.

use adi_config::Config;

use crate::types::{SetUsageEvents, UsageEventsState};

use super::response::{Response, ok_json};

/// `GET /api/usage-events`.
#[must_use]
pub fn usage_events_state(cfg: &Config) -> Response {
    ok_json(&snapshot(cfg))
}

/// `POST /api/usage-events/toggle` — set [`UsageEventsState::persisted`] and answer with the
/// fresh state. Never mints or clears `install_id`: turning usage events off leaves an existing
/// one on disk, dormant, exactly as the plan requires (§7) — only [`forget_usage_events`] clears
/// it.
#[must_use]
pub fn set_usage_events(cfg: &Config, body: &[u8]) -> Response {
    let req = require!(body, SetUsageEvents);
    let module = adi_usage::module(cfg);
    if let Err(e) = adi_usage::set_enabled(&module, req.enabled) {
        return super::response::error(500, &format!("saving the usage-events setting: {e}"));
    }
    ok_json(&snapshot(cfg))
}

/// `POST /api/usage-events/forget` — `adi-mono analytics forget`'s panel counterpart: clear
/// `install_id` (and the rest of this install's usage-events state). A fresh id is minted only
/// the next time usage events run while on, never by this call.
#[must_use]
pub fn forget_usage_events(cfg: &Config) -> Response {
    let module = adi_usage::module(cfg);
    if let Err(e) = adi_usage::forget(&module) {
        return super::response::error(500, &format!("forgetting the install id: {e}"));
    }
    ok_json(&snapshot(cfg))
}

fn snapshot(cfg: &Config) -> UsageEventsState {
    let module = adi_usage::module(cfg);
    let persisted = adi_usage::Settings::load(&module).enabled;
    let env_override = adi_usage::env_override().map(|(name, _)| name.to_string());
    UsageEventsState {
        enabled: adi_usage::effective(&module),
        persisted,
        env_override,
        install_id: adi_usage::State::load(&module).install_id,
    }
}

impl super::response::FromBody for SetUsageEvents {
    const EXPECTED: &'static str = "expected JSON body { \"enabled\": true|false }";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-usage-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn body(cfg: &Config, path: fn(&Config) -> Response) -> UsageEventsState {
        let res = path(cfg);
        assert_eq!(res.status, 200, "{}", res.body);
        serde_json::from_str(&res.body).expect("usage-events state")
    }

    #[test]
    fn a_fresh_store_reads_as_the_crate_default_with_no_install_id() {
        let cfg = scratch("default");
        let state = body(&cfg, usage_events_state);
        assert_eq!(state.enabled, adi_usage::DEFAULT_ENABLED);
        assert_eq!(state.persisted, adi_usage::DEFAULT_ENABLED);
        assert_eq!(state.env_override, None);
        assert_eq!(state.install_id, None);
    }

    #[test]
    fn toggling_persists_and_reads_back_without_touching_the_install_id() {
        let cfg = scratch("toggle");
        let module = adi_usage::module(&cfg);
        adi_usage::State {
            install_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
            first_launch_at: 1,
            ..Default::default()
        }
        .save(&module)
        .expect("seed an install id");

        let on = set_usage_events(&cfg, br#"{"enabled":true}"#);
        let state: UsageEventsState = serde_json::from_str(&on.body).expect("state");
        assert!(state.enabled);
        assert!(state.persisted);
        assert_eq!(state.install_id.as_deref(), Some("11111111-1111-1111-1111-111111111111"));

        let off = set_usage_events(&cfg, br#"{"enabled":false}"#);
        let state: UsageEventsState = serde_json::from_str(&off.body).expect("state");
        assert!(!state.enabled);
        assert_eq!(
            state.install_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111"),
            "switching off must not clear an existing install id"
        );
    }

    #[test]
    fn forget_clears_the_install_id() {
        let cfg = scratch("forget");
        let module = adi_usage::module(&cfg);
        adi_usage::State {
            install_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
            first_launch_at: 1,
            ..Default::default()
        }
        .save(&module)
        .expect("seed");

        let state = body(&cfg, forget_usage_events);
        assert_eq!(state.install_id, None);
    }

    #[test]
    fn a_malformed_body_is_a_400() {
        let cfg = scratch("bad-body");
        let res = set_usage_events(&cfg, b"not json");
        assert_eq!(res.status, 400);
    }
}
