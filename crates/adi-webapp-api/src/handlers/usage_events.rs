//! Three bus events this crate's own handlers publish directly, for `adi-usage` (ADI-MONO-157) to
//! pick up — none of this crate's three producers (dashboards, marketplace, fleet) had one before.
//!
//! Published with [`adi_events::Events::emit_json`], which swallows both the encoding and the
//! append failure — the same "a notification must never fail the mutation it describes" rule
//! every other publisher on the bus follows (see that function's own doc comment). Each handler
//! builds its own [`adi_config::Config`] handle from the one it was already given, rather than
//! taking an `Events` parameter: these are one-line, fire-and-forget sends, not a dependency
//! worth threading through every call site above them.

use adi_config::Config;
use serde::Serialize;

/// `adi.dashboards.created` — a dashboard was scaffolded (`create_dashboard`) or landed by an
/// import (`import_dashboard`).
#[derive(Serialize)]
struct DashboardCreated {
    /// `blank` for a fresh scaffold, `import` for a bundle landed from another machine.
    from: &'static str,
}

/// `adi.marketplace.installed` — one element of a marketplace install landed:
/// `install_marketplace_app`'s legacy single-dashboard path fires this once, a general bundle
/// once per element that actually landed.
#[derive(Serialize)]
struct MarketplaceInstalled<'a> {
    /// The source's local name — `store` for the ADI Store, anything else for a marketplace the
    /// operator configured by hand (`adi_marketplace::sources::OFFICIAL_NAME`, not imported here
    /// to avoid a dependency only this one string needs).
    marketplace: &'a str,
    /// The bundle's published slug.
    slug: &'a str,
    /// `adi_marketplace::Kind::wire()` — `agent` | `tool` | `dashboard` | `llm` | `embedding` |
    /// `service` | `trigger` | `project`. Published for all eight; the collector's dictionary
    /// only has a `kind` value for four of them, so `adi-usage` is where the other four are
    /// dropped, not here — this event describes what happened, not what is allowed onto the wire.
    kind: &'a str,
}

/// `adi.fleet.invite.created` — a fleet/mesh invite was minted.
#[derive(Serialize)]
struct FleetInviteCreated {}

fn publish(cfg: &Config, name: &str, payload: &impl Serialize) {
    adi_events::Events::with_config(cfg.clone()).emit_json(name, payload);
}

pub(super) fn dashboard_created(cfg: &Config, from: &'static str) {
    publish(cfg, "adi.dashboards.created", &DashboardCreated { from });
}

pub(super) fn marketplace_installed(cfg: &Config, marketplace: &str, slug: &str, kind: &str) {
    publish(
        cfg,
        "adi.marketplace.installed",
        &MarketplaceInstalled { marketplace, slug, kind },
    );
}

pub(super) fn fleet_invite_created(cfg: &Config) {
    publish(cfg, "adi.fleet.invite.created", &FleetInviteCreated {});
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_events::Events;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-usage-events-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    #[test]
    fn each_event_lands_on_the_bus_with_its_own_payload() {
        let cfg = scratch("roundtrip");
        dashboard_created(&cfg, "import");
        marketplace_installed(&cfg, "store", "crm-suite", "dashboard");
        fleet_invite_created(&cfg);

        let bus = Events::with_config(cfg);
        let caught_up = bus.catch_up("test", 10).expect("catch up");
        assert_eq!(caught_up.len(), 3);
        assert_eq!(caught_up[0].record.name, "adi.dashboards.created");
        assert_eq!(caught_up[0].record.payload, r#"{"from":"import"}"#);
        assert_eq!(caught_up[1].record.name, "adi.marketplace.installed");
        assert_eq!(
            caught_up[1].record.payload,
            r#"{"marketplace":"store","slug":"crm-suite","kind":"dashboard"}"#
        );
        assert_eq!(caught_up[2].record.name, "adi.fleet.invite.created");
        assert_eq!(caught_up[2].record.payload, "{}");
    }
}
