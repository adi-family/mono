//! `/api/marketplace*` — the panel's door onto the apps marketplace: the listing (sources and
//! cached entries, no network), the explicit sync, and the two deliberate acts — install and
//! start.
//!
//! The listing answers from the store alone so the page renders offline; sync is the only
//! endpoint that fetches, and it reports per source what happened (the same summaries the CLI
//! prints). Install and start are thin: the rules live in `adi-marketplace`, and every refusal
//! arrives here already phrased for a person.

use adi_config::Config;
use adi_marketplace::Marketplace;

use crate::types::{
    InstallMarketplaceApp, MarketplaceApp, MarketplaceDone, MarketplaceSource, MarketplaceState,
    StartMarketplaceApp,
};

use super::response::{Response, error, ok_json};

/// The whole state as the panel renders it, read from the store with no network.
#[must_use]
pub fn state(market: &Marketplace) -> MarketplaceState {
    MarketplaceState {
        sources: adi_marketplace::source_states(market.config())
            .into_iter()
            .map(|s| MarketplaceSource {
                name: s.name,
                url: s.url,
                synced_at: s.synced_at,
                error: s.error,
            })
            .collect(),
        apps: adi_marketplace::install::cached_apps(market.config())
            .into_iter()
            .map(|a| MarketplaceApp {
                marketplace: a.marketplace,
                slug: a.slug,
                name: a.name,
                description: a.description,
                version: a.version,
                installed: a.installed,
                started: a.started,
                host: a.host,
            })
            .collect(),
    }
}

/// `GET /api/marketplace` — sources and cached entries.
#[must_use]
pub fn marketplace(cfg: &Config) -> Response {
    ok_json(&state(&Marketplace::with_config(cfg.clone())))
}

/// `POST /api/marketplace/sync` — fetch every source, then answer with the fresh state and one
/// line per source saying what happened. A source that failed with a cache to fall back on is a
/// warning carried in the message, not an error: the stale listing still rendered.
#[must_use]
pub fn sync_marketplace(cfg: &Config) -> Response {
    let market = Marketplace::with_config(cfg.clone());
    let results = match adi_marketplace::sync::sync(&market) {
        Ok(results) => results,
        Err(e) => return error(500, &e.to_string()),
    };
    let failed: Vec<&adi_marketplace::sync::SyncResult> =
        results.iter().filter(|r| !r.has_listing()).collect();
    let message = if results.is_empty() {
        "no marketplaces configured — add one with `adi-mono marketplace add`".to_string()
    } else {
        results
            .iter()
            .map(adi_marketplace::sync::SyncResult::summary)
            .collect::<Vec<_>>()
            .join(" · ")
    };
    if !failed.is_empty() {
        // Sources with nothing to show are a real failure, not a degraded listing.
        return error(502, &message);
    }
    ok_json(&MarketplaceDone {
        state: state(&market),
        message,
    })
}

/// `POST /api/marketplace/install` — land an entry's artifact in the dashboard store, started
/// nothing. The refusal a collision earns is a 409, so the page can offer `force` as the way
/// past it rather than a generic failure.
#[must_use]
pub fn install_marketplace_app(cfg: &Config, body: &[u8]) -> Response {
    let req: InstallMarketplaceApp = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let spec = format!("{}/{}", req.marketplace, req.slug);
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::install::install(&market, &spec, req.force) {
        Ok(done) => {
            let message = if done.started {
                format!("reinstalled {} — kept running at {}", done.slug, done.host)
            } else {
                format!(
                    "installed {} — not started. Start is the act that runs it.",
                    done.slug
                )
            };
            ok_json(&MarketplaceDone {
                state: state(&market),
                message,
            })
        }
        Err(e) => refusal(&e),
    }
}

/// `POST /api/marketplace/start` — move an installed app into the supervisor's glob.
#[must_use]
pub fn start_marketplace_app(cfg: &Config, body: &[u8]) -> Response {
    let req: StartMarketplaceApp = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::install::start(&market, &req.slug) {
        Ok(started) => {
            let message = format!(
                "started {} — http://{} , once the servers have come up",
                started.slug, started.host
            );
            ok_json(&MarketplaceDone {
                state: state(&market),
                message,
            })
        }
        Err(e) => refusal(&e),
    }
}

/// Map a marketplace refusal onto its HTTP shape: a collision is a 409 (the page offers `force`),
/// an unknown name is a 404, and everything else about the ask was malformed.
fn refusal(e: &adi_marketplace::Error) -> Response {
    use adi_marketplace::Error as E;
    let status = match e {
        E::UnknownSource(_) | E::UnknownApp { .. } | E::NotInstalled(_) => 404,
        E::NotSynced(_) | E::Duplicate(_) | E::Collision { .. } => 409,
        E::InvalidName(_) | E::NotHttps(_) | E::BadSpec(_) | E::BadSlug(_) => 400,
        E::BadArtifact { .. } | E::Fetch(_) => 502,
        E::Config(_) | E::Io(_) => 500,
    };
    error(status, &e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store of this test's own, under the system temp dir — never the operator's live one.
    fn store(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-marketplace-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    /// A source added, its cache seeded with a one-app manifest. The artifact URL names a host
    /// no test can reach — these tests exercise the door's HTTP shapes, and the landing the
    /// artifact goes to is `adi-marketplace`'s own, tested there with an injectable fetch.
    fn seeded(tag: &str) -> Config {
        let cfg = store(tag);
        adi_marketplace::sources::add(&cfg, "adi", "https://example/m.json").expect("add");
        write_cache(&cfg, None);
        cfg
    }

    /// Seed `adi`'s cache envelope, optionally carrying a fetch-failure note.
    fn write_cache(cfg: &Config, error: Option<&str>) {
        let note = error
            .map(|e| format!(",\"error\":{e:?}"))
            .unwrap_or_default();
        cfg.module("marketplace")
            .write_raw(
                "cache/adi.json",
                format!(
                    "{{\"url\":\"https://example/m.json\",\"fetched_at\":1788288916{note},\"manifest\":{{\
                     \"name\":\"ADI starter apps\",\"apps\": [\
                       {{\"slug\":\"crm\",\"name\":\"CRM\",\"version\":\"0.1.0\",\
                        \"artifact\":\"https://example/crm.bundle.json\"}}]}}}}"
                )
                .as_bytes(),
            )
            .expect("cache");
    }

    /// The fixture bundle an install "fetches", packed the way an export packs one.
    fn bundle() -> Vec<u8> {
        use base64::Engine as _;
        let contents = base64::engine::general_purpose::STANDARD.encode(b"export default () => 1;");
        format!(
            r#"{{"id":"x","name":"CRM","host":"crm.adi","files":[{{"path":"frontend/index.ts","contents":"{contents}"}}]}}"#
        )
        .into_bytes()
    }

    /// Land the CRM the way a successful install leaves it — through the crate's own
    /// injectable-fetch seam, so no test touches the network.
    fn land(cfg: &Config) {
        adi_marketplace::install::install_with(
            &Marketplace::with_config(cfg.clone()),
            "adi/crm",
            false,
            |_| Ok(bundle()),
        )
        .expect("install");
    }

    #[test]
    fn the_listing_reads_the_store_and_says_when_it_is_stale() {
        let cfg = seeded("listing");

        let res = marketplace(&cfg);
        assert_eq!(res.status, 200, "{}", res.body);
        let v: serde_json::Value = serde_json::from_str(&res.body).expect("json");
        assert_eq!(v["sources"][0]["name"], "adi");
        assert_eq!(v["sources"][0]["synced_at"], 1_788_288_916);
        assert_eq!(v["apps"][0]["slug"], "crm");
        assert_eq!(v["apps"][0]["installed"], false);
        assert_eq!(v["apps"][0]["started"], false);
        // No count of anything rides on the payload.
        for key in ["count", "installs", "total"] {
            assert!(v.get(key).is_none(), "{key} must not appear: {}", res.body);
        }

        // A failed fetch recorded in the envelope surfaces as the stale sentence.
        write_cache(&cfg, Some("dns went away"));
        let v: serde_json::Value = serde_json::from_str(&marketplace(&cfg).body).expect("json");
        assert_eq!(v["sources"][0]["error"], "dns went away");
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn install_refusals_keep_their_shapes() {
        let cfg = seeded("refusals");

        assert_eq!(
            install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"nope"}"#).status,
            404,
            "an app the manifest does not carry"
        );
        assert_eq!(
            install_marketplace_app(&cfg, b"not json").status,
            400,
            "a malformed body"
        );
        assert_eq!(
            install_marketplace_app(&cfg, br#"{"marketplace":"ghost","slug":"crm"}"#).status,
            404,
            "a marketplace nothing here knows"
        );

        // A dashboard already answering to the slug is the collision the page offers force past.
        land(&cfg);
        let res = install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"crm"}"#);
        assert_eq!(res.status, 409, "{}", res.body);
        assert!(res.body.contains("--force"), "{}", res.body);

        // A source with no cache has nothing to install from.
        let bare = store("refusals-bare");
        adi_marketplace::sources::add(&bare, "adi", "https://example/m.json").expect("add");
        assert_eq!(
            install_marketplace_app(&bare, br#"{"marketplace":"adi","slug":"crm"}"#).status,
            409
        );
        let _ = std::fs::remove_dir_all(cfg.root());
        let _ = std::fs::remove_dir_all(bare.root());
    }

    #[test]
    fn start_goes_through_the_door_and_flips_the_state() {
        let cfg = seeded("start");
        land(&cfg);
        assert!(
            !cfg.module("dashboards")
                .dir()
                .join("crm")
                .join(".adi")
                .join("hive.yaml")
                .exists(),
            "precondition: installed is not started"
        );

        let res = start_marketplace_app(&cfg, br#"{"slug":"crm"}"#);
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(done.message.contains("started crm"), "{}", done.message);
        assert_eq!(done.state.apps[0].installed, true);
        assert_eq!(done.state.apps[0].started, true);
        assert_eq!(done.state.apps[0].host.as_deref(), Some("crm.adi"));
        assert!(
            cfg.module("dashboards")
                .dir()
                .join("crm")
                .join(".adi")
                .join("hive.yaml")
                .exists(),
            "the hive file is in the supervisor's glob now"
        );
        assert_eq!(
            start_marketplace_app(&cfg, br#"{"slug":"ghost"}"#).status,
            404
        );
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn a_sync_of_an_empty_store_says_so_rather_than_erroring() {
        let cfg = store("empty");
        let res = sync_marketplace(&cfg);
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(
            res.body.contains("no marketplaces configured"),
            "{}",
            res.body
        );
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn the_refusal_map_gives_each_failure_its_own_shape() {
        use adi_marketplace::Error as E;
        let status = |e: &E| refusal(e).status;
        assert_eq!(status(&E::Collision("crm".into(), "/d".into())), 409);
        assert_eq!(status(&E::UnknownSource("x".into())), 404);
        assert_eq!(
            status(&E::UnknownApp("adi".into(), "crm".into(), "nosh".into())),
            404
        );
        assert_eq!(status(&E::NotInstalled("crm".into())), 404);
        assert_eq!(status(&E::NotSynced("adi".into())), 409);
        assert_eq!(status(&E::BadSpec("crm".into())), 400);
        assert_eq!(status(&E::BadSlug("../x".into())), 400);
        assert_eq!(status(&E::BadArtifact("crm".into(), "nope".into())), 502);
        assert_eq!(status(&E::Fetch("unreachable".into())), 502);
        assert_eq!(status(&E::Duplicate("adi".into())), 409);
    }
}
