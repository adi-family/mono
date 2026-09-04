//! `/api/marketplace*` — the panel's door onto the apps marketplace: the listing (sources and
//! cached entries, no network), the explicit sync, and the three deliberate acts — install, start
//! and update.
//!
//! The listing answers from the store alone so the page renders offline; sync is the only
//! endpoint that fetches a manifest, and it reports per source what happened (the same summaries
//! the CLI prints). Install clones the entry's repository at the commit it pins, under the name
//! the operator chose. These handlers are thin: the rules live in `adi-marketplace`, and every
//! refusal arrives here already phrased for a person.

use adi_config::Config;
use adi_marketplace::Marketplace;

use crate::types::{
    InstallMarketplaceApp, MarketplaceApp, MarketplaceDone, MarketplaceInstall, MarketplaceSource,
    MarketplaceState, StartMarketplaceApp, UpdateMarketplaceApp,
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
                repo: a.repo,
                commit: a.commit,
                branch: a.branch,
                installs: a
                    .installs
                    .into_iter()
                    .map(|i| MarketplaceInstall {
                        id: i.id,
                        name: i.name,
                        commit: i.commit,
                        started: i.started,
                        host: i.host,
                        outdated: i.outdated,
                    })
                    .collect(),
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

/// `POST /api/marketplace/install` — clone the entry's repository at its pinned commit as a
/// dashboard called whatever the operator typed, and start it only if they asked.
///
/// Nothing here can collide: the id is minted from the name the way every store id is, so a
/// second copy of one app is `crm-2` rather than a refusal.
#[must_use]
pub fn install_marketplace_app(cfg: &Config, body: &[u8]) -> Response {
    let req: InstallMarketplaceApp = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let spec = format!("{}/{}", req.marketplace, req.slug);
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::install::install(&market, &spec, &req.name, req.start) {
        Ok(done) => {
            // Where it went, said in full when it is not running: an unstarted dashboard is filed
            // under Archived, and somebody who just installed one will not think to look there.
            let where_it_is = if done.started {
                format!("running at http://{}", done.host)
            } else {
                "not started — press Start below, or find it under Archived on the Dashboards page"
                    .to_string()
            };
            let mut message = format!(
                "installed “{}” as {} at {} — {where_it_is}",
                done.name,
                done.id,
                adi_marketplace::git::short(&done.commit),
            );
            for note in &done.notes {
                message.push_str(" · ");
                message.push_str(note);
            }
            ok_json(&MarketplaceDone {
                state: state(&market),
                message,
            })
        }
        Err(e) => refusal(&e),
    }
}

/// `POST /api/marketplace/start` — move an installed copy into the supervisor's glob.
#[must_use]
pub fn start_marketplace_app(cfg: &Config, body: &[u8]) -> Response {
    let req: StartMarketplaceApp = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::install::start(&market, &req.id) {
        Ok(started) => {
            let message = format!(
                "started {} — http://{} , once the servers have come up",
                started.id, started.host
            );
            ok_json(&MarketplaceDone {
                state: state(&market),
                message,
            })
        }
        Err(e) => refusal(&e),
    }
}

/// `POST /api/marketplace/update` — fast-forward an installed copy onto the commit its
/// marketplace now pins. Uncommitted work is a 409, so the page can offer `force` as the way past
/// it and say what forcing costs.
#[must_use]
pub fn update_marketplace_app(cfg: &Config, body: &[u8]) -> Response {
    let req: UpdateMarketplaceApp = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::install::update(&market, &req.id, req.force) {
        Ok(done) => {
            let short = adi_marketplace::git::short;
            let message = if done.changed {
                let reload = if done.started {
                    " — it is running, so bun has already reloaded it"
                } else {
                    ""
                };
                format!(
                    "updated {} from {} to {}{reload}",
                    done.id,
                    short(&done.from),
                    short(&done.to)
                )
            } else {
                format!(
                    "{} is already at {}, the commit its marketplace pins",
                    done.id,
                    short(&done.to)
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

/// Map a marketplace refusal onto its HTTP shape: work that would be lost is a 409 (the page
/// offers `force`), an unknown name is a 404, a manifest or a repository that does not belong is
/// a 502 — it is somebody else's server that is wrong — and everything else about the ask was
/// malformed.
fn refusal(e: &adi_marketplace::Error) -> Response {
    use adi_marketplace::Error as E;
    let status = match e {
        E::UnknownSource(_) | E::UnknownApp { .. } | E::NotInstalled(_) => 404,
        E::NotSynced(_) | E::Duplicate(_) | E::Dirty(_) => 409,
        E::InvalidName(_) | E::NotHttps(_) | E::BadSpec(_) | E::EmptyName => 400,
        // Everything a publisher got wrong, and everything git or the network refused: the ask
        // was fine, the far end was not.
        E::BadSlug(_)
        | E::BadRepo(_)
        | E::BadCommit { .. }
        | E::NotAnApp { .. }
        | E::Git(_)
        | E::Fetch(_) => 502,
        E::Config(_) | E::Io(_) => 500,
    };
    error(status, &e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    /// A real repository standing in for what a publisher hosts: dashboard-shaped, one commit.
    /// Answers its `file://` URL and that commit.
    fn upstream(root: &Path) -> (String, String) {
        let dir = root.join("upstream");
        std::fs::create_dir_all(dir.join("frontend")).expect("frontend");
        std::fs::create_dir_all(dir.join("backend")).expect("backend");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "--quiet", "-b", "main"]);
        git(&["config", "user.email", "publisher@example"]);
        git(&["config", "user.name", "The Publisher"]);
        std::fs::write(dir.join("frontend").join("index.ts"), "// front\n").expect("seed");
        std::fs::write(dir.join("backend").join("index.ts"), "// back\n").expect("seed");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "v1"]);
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse");
        (
            format!("file://{}", dir.display()),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    }

    /// A source added and its cache seeded with a one-app manifest pinning the fixture repo.
    fn seeded(tag: &str) -> (Config, String) {
        let cfg = store(tag);
        let (repo, commit) = upstream(&cfg.root().join("publisher"));
        adi_marketplace::sources::add(&cfg, "adi", "https://example/m.json").expect("add");
        write_cache(&cfg, &repo, &commit, None);
        (cfg, commit)
    }

    /// Seed `adi`'s cache envelope, optionally carrying a fetch-failure note.
    fn write_cache(cfg: &Config, repo: &str, commit: &str, error: Option<&str>) {
        let note = error
            .map(|e| format!(",\"error\":{e:?}"))
            .unwrap_or_default();
        cfg.module("marketplace")
            .write_raw(
                "cache/adi.json",
                format!(
                    "{{\"url\":\"https://example/m.json\",\"fetched_at\":1788288916{note},\
                     \"manifest\":{{\"name\":\"ADI starter apps\",\"apps\":[\
                       {{\"slug\":\"crm\",\"name\":\"CRM\",\"version\":\"0.1.0\",\
                        \"repo\":{repo:?},\"commit\":{commit:?}}}]}}}}"
                )
                .as_bytes(),
            )
            .expect("cache");
    }

    #[test]
    fn the_listing_reads_the_store_and_says_when_it_is_stale() {
        let (cfg, commit) = seeded("listing");

        let res = marketplace(&cfg);
        assert_eq!(res.status, 200, "{}", res.body);
        let v: serde_json::Value = serde_json::from_str(&res.body).expect("json");
        assert_eq!(v["sources"][0]["name"], "adi");
        assert_eq!(v["sources"][0]["synced_at"], 1_788_288_916);
        assert_eq!(v["apps"][0]["slug"], "crm");
        assert_eq!(v["apps"][0]["commit"], commit);
        assert_eq!(
            v["apps"][0]["installs"].as_array().map(Vec::len),
            Some(0),
            "nothing installed yet"
        );
        // No count of anything rides on the payload.
        for key in ["count", "installs_total", "total"] {
            assert!(v.get(key).is_none(), "{key} must not appear: {}", res.body);
        }

        // A failed fetch recorded in the envelope surfaces as the stale sentence.
        let repo = v["apps"][0]["repo"].as_str().expect("repo").to_string();
        write_cache(&cfg, &repo, &commit, Some("dns went away"));
        let v: serde_json::Value = serde_json::from_str(&marketplace(&cfg).body).expect("json");
        assert_eq!(v["sources"][0]["error"], "dns went away");
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn an_install_takes_the_name_it_was_given_and_starts_nothing() {
        let (cfg, commit) = seeded("install");

        let res = install_marketplace_app(
            &cfg,
            br#"{"marketplace":"adi","slug":"crm","name":"Sales CRM"}"#,
        );
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(done.message.contains("Sales CRM"), "{}", done.message);
        assert!(done.message.contains("not started"), "{}", done.message);
        let install = &done.state.apps[0].installs[0];
        assert_eq!(install.id, "sales-crm");
        assert_eq!(install.commit, commit);
        assert!(!install.started);
        assert!(!install.outdated);

        // A second copy is ordinary, and it is a second row rather than a refusal.
        let res = install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"crm"}"#);
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert_eq!(done.state.apps[0].installs.len(), 2, "{:?}", done.state.apps);
        assert!(
            done.state.apps[0].installs.iter().any(|i| i.id == "crm"),
            "an unnamed copy takes the entry's own name: {:?}",
            done.state.apps[0].installs
        );
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn install_refusals_keep_their_shapes() {
        let (cfg, _commit) = seeded("refusals");

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
        let (cfg, _commit) = seeded("start");
        let landed =
            install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"crm","name":"CRM"}"#);
        assert_eq!(landed.status, 200, "{}", landed.body);
        let live = cfg
            .module("dashboards")
            .dir()
            .join("crm")
            .join(".adi")
            .join("hive.yaml");
        assert!(!live.exists(), "precondition: installed is not started");

        let res = start_marketplace_app(&cfg, br#"{"id":"crm"}"#);
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(done.message.contains("started crm"), "{}", done.message);
        assert!(done.state.apps[0].installs[0].started);
        assert_eq!(
            done.state.apps[0].installs[0].host.as_deref(),
            Some("crm.adi")
        );
        assert!(live.exists(), "the hive file is in the supervisor's glob now");
        assert_eq!(start_marketplace_app(&cfg, br#"{"id":"ghost"}"#).status, 404);
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn an_update_answers_even_when_there_is_nothing_to_do() {
        let (cfg, commit) = seeded("update");
        let landed =
            install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"crm","name":"CRM"}"#);
        assert_eq!(landed.status, 200, "{}", landed.body);

        let res = update_marketplace_app(&cfg, br#"{"id":"crm"}"#);
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(
            done.message.contains("already at"),
            "the pin it is on is not a failure: {}",
            done.message
        );
        assert!(
            done.message.contains(&commit[..7]),
            "and it says which: {}",
            done.message
        );
        assert_eq!(update_marketplace_app(&cfg, br#"{"id":"ghost"}"#).status, 404);
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
        assert_eq!(status(&E::Dirty("crm".into())), 409);
        assert_eq!(status(&E::UnknownSource("x".into())), 404);
        assert_eq!(
            status(&E::UnknownApp("adi".into(), "crm".into(), "nosh".into())),
            404
        );
        assert_eq!(status(&E::NotInstalled("crm".into())), 404);
        assert_eq!(status(&E::NotSynced("adi".into())), 409);
        assert_eq!(status(&E::BadSpec("crm".into())), 400);
        assert_eq!(status(&E::EmptyName), 400);
        assert_eq!(status(&E::BadSlug("../x".into())), 502);
        assert_eq!(status(&E::BadRepo("git://x".into())), 502);
        assert_eq!(status(&E::BadCommit("crm".into(), "main".into())), 502);
        assert_eq!(status(&E::NotAnApp("crm".into(), "no frontend".into())), 502);
        assert_eq!(status(&E::Git("clone failed".into())), 502);
        assert_eq!(status(&E::Fetch("unreachable".into())), 502);
        assert_eq!(status(&E::Duplicate("adi".into())), 409);
    }
}
