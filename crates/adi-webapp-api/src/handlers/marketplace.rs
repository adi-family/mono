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
use adi_marketplace::{Kind, Marketplace};

use crate::types::{
    InstallMarketplaceApp, MarketplaceApp, MarketplaceBundleElement, MarketplaceBundleStatus,
    MarketplaceDone, MarketplaceElementPreview, MarketplaceInstall, MarketplaceMedia,
    MarketplaceMediaKind, MarketplaceSource, MarketplaceState, StartMarketplaceApp,
    StartMarketplaceService, UninstallMarketplaceElement, UpdateMarketplaceApp,
    UpdateMarketplaceBundle,
};

use super::response::{Response, error, ok_json};

/// The whole state as the panel renders it, read from the store with no network.
#[must_use]
pub fn state(market: &Marketplace) -> MarketplaceState {
    let config = market.config();
    MarketplaceState {
        sources: adi_marketplace::source_states(config)
            .into_iter()
            .map(|s| MarketplaceSource {
                name: s.name,
                url: s.url,
                synced_at: s.synced_at,
                error: s.error,
            })
            .collect(),
        apps: adi_marketplace::install::cached_apps(config)
            .into_iter()
            .map(|a| {
                // The manifest's own preview and this bundle's live status both read off the same
                // cached entry — one more lookup, from the cache and never the network, the same
                // cost `cached_apps` itself already paid to build the row this is joining.
                let entry = adi_marketplace::entry_of(config, &a.marketplace, &a.slug).ok();
                let elements = entry
                    .as_ref()
                    .map(|e| {
                        e.elements()
                            .into_iter()
                            .map(|(kind, el)| MarketplaceElementPreview {
                                kind: kind.wire().to_string(),
                                name: el.name().to_string(),
                                description: el.description().map(str::to_string),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let bundle = entry
                    .as_ref()
                    .and_then(|e| adi_marketplace::bundle::status(market, &a.marketplace, &a.slug, e))
                    .map(bundle_status);
                MarketplaceApp {
                    marketplace: a.marketplace,
                    slug: a.slug,
                    name: a.name,
                    description: a.description,
                    icon: a.icon,
                    keywords: a.keywords,
                    readme: a.readme,
                    gallery: a.gallery.into_iter().map(media).collect(),
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
                    elements,
                    bundle,
                }
            })
            .collect(),
    }
}

/// A general bundle's status, onto the wire shape.
fn bundle_status(s: adi_marketplace::BundleStatus) -> MarketplaceBundleStatus {
    MarketplaceBundleStatus {
        declared: s.declared,
        outdated: s.outdated,
        missing_secrets: s.missing_secrets,
        // `kind.to_string()`, not `kind.wire()`: this is the address spelling
        // (`crate::Kind::dir`), because the panel builds `<kind>/<name>` straight off this field
        // to install, uninstall or start the element it names.
        elements: s
            .elements
            .into_iter()
            .map(|e| MarketplaceBundleElement {
                kind: e.kind.to_string(),
                name: e.name,
                description: e.description,
                id: e.id,
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

/// `POST /api/marketplace/install` — install the whole bundle, or one `<kind>/<name>` element of
/// it. A legacy single-dashboard bundle installs exactly as v1 always has — a dashboard called
/// whatever the operator typed, started only if they asked; a general bundle lands every selected
/// element under its own published name (`docs/marketplace-bundles.md`).
///
/// Nothing about a legacy install can collide: the id is minted from the name the way every store
/// id is, so a second copy of one app is `crm-2` rather than a refusal. A general bundle's own
/// collision rules are per element — see [`install_message`].
#[must_use]
pub fn install_marketplace_app(cfg: &Config, body: &[u8]) -> Response {
    let req: InstallMarketplaceApp = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let spec = spec_of(&req.marketplace, &req.slug, req.element.as_deref());
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::bundle::install(&market, &spec, &req.name, req.start) {
        Ok(done) => ok_json(&MarketplaceDone {
            message: install_message(&done),
            state: state(&market),
        }),
        Err(e) => refusal(&e),
    }
}

/// `<marketplace>/<slug>`, or `<marketplace>/<slug>/<element>` when the request names one — the
/// address [`adi_marketplace::bundle::install`] (and every other bundle verb below) reads.
fn spec_of(marketplace: &str, slug: &str, element: Option<&str>) -> String {
    match element.map(str::trim).filter(|e| !e.is_empty()) {
        Some(element) => format!("{marketplace}/{slug}/{element}"),
        None => format!("{marketplace}/{slug}"),
    }
}

/// The line an install answers with — the legacy dashboard's own sentence, unchanged, or one line
/// per element a general bundle considered.
fn install_message(done: &adi_marketplace::BundleOutcome) -> String {
    use adi_marketplace::BundleOutcome;
    match done {
        BundleOutcome::Legacy(done) => {
            // Where it went, said in full when it is not running — an install nobody can find is
            // an install that did not happen.
            let where_it_is = if done.started {
                format!("running at http://{}", done.host)
            } else {
                "not started — press Start, here or on its row on the Dashboards page".to_string()
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
            message
        }
        BundleOutcome::Bundle(done) => {
            let mut message = format!(
                "{}/{}: {}",
                done.marketplace,
                done.slug,
                done.elements
                    .iter()
                    .map(element_outcome_line)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for note in &done.notes {
                message.push_str(" · ");
                message.push_str(note);
            }
            message
        }
    }
}

/// One element's own outcome, as a phrase: `agents/sales-bot → sales-bot`, or the note when it
/// did not land.
fn element_outcome_line(e: &adi_marketplace::ElementOutcome) -> String {
    match (&e.id, &e.note) {
        (Some(id), _) if !e.renamed => format!("{}/{} → {id}", e.kind, e.name),
        (Some(id), _) => format!("{}/{} → {id} (renamed)", e.kind, e.name),
        (None, Some(note)) => format!("{}/{}: {note}", e.kind, e.name),
        (None, None) => format!("{}/{}: did not land", e.kind, e.name),
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

/// `POST /api/marketplace/uninstall` — remove one installed element of a bundle by its own kind's
/// ordinary path, and drop it from the ledger. Every sibling is left untouched.
#[must_use]
pub fn uninstall_marketplace_element(cfg: &Config, body: &[u8]) -> Response {
    let req: UninstallMarketplaceElement = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let spec = spec_of(&req.marketplace, &req.slug, Some(&req.element));
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::bundle::uninstall_element(&market, &spec) {
        Ok(done) => {
            let mut message = format!("uninstalled {}/{} ({})", done.kind, done.name, done.id);
            if done.bundle_removed {
                message.push_str(" — the last element from this bundle, so its own record is gone too");
            }
            if let Some(note) = &done.note {
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

/// `POST /api/marketplace/start-service` — copy a parked hive service element's block into the
/// live hive.yaml it belongs in.
#[must_use]
pub fn start_marketplace_service(cfg: &Config, body: &[u8]) -> Response {
    let req: StartMarketplaceService = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let spec = spec_of(&req.marketplace, &req.slug, Some(&format!("{}/{}", Kind::Service, req.name)));
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::bundle::start_service(&market, &spec) {
        Ok(started) => {
            let where_ = match &started.project {
                Some(project) => format!("into {project}'s own hive.yaml"),
                None => "into the global hive.yaml".to_string(),
            };
            let message = format!(
                "started {} ({}) — copied {where_}; the supervisor picks it up on its next read",
                started.name, started.id
            );
            ok_json(&MarketplaceDone {
                state: state(&market),
                message,
            })
        }
        Err(e) => refusal(&e),
    }
}

/// `POST /api/marketplace/bundle/update` — fast-forward a general bundle's own internal clone onto
/// its manifest's current pin, and re-apply every ledgered element, per element rather than
/// all-or-nothing. Every `<kind>/<name>` in `force` is overwritten past its own local edit; every
/// other ledgered element fast-forwards, or is left alone and reported.
#[must_use]
pub fn update_marketplace_bundle(cfg: &Config, body: &[u8]) -> Response {
    let req: UpdateMarketplaceBundle = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let force: Vec<(Kind, &str)> = req.force.iter().filter_map(|f| force_pair(f)).collect();
    let market = Marketplace::with_config(cfg.clone());
    match adi_marketplace::bundle::update(&market, &req.marketplace, &req.slug, &force) {
        Ok(done) => {
            let short = adi_marketplace::git::short;
            let mut message = if done.changed {
                format!(
                    "{}/{} updated from {} to {}: ",
                    done.marketplace,
                    done.slug,
                    short(&done.from),
                    short(&done.to)
                )
            } else {
                format!(
                    "{}/{} is already at {}, the commit its marketplace pins",
                    done.marketplace,
                    done.slug,
                    short(&done.to)
                )
            };
            if done.changed {
                message.push_str(
                    &done
                        .elements
                        .iter()
                        .map(element_update_line)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            ok_json(&MarketplaceDone {
                state: state(&market),
                message,
            })
        }
        Err(e) => refusal(&e),
    }
}

/// `<kind>/<name>` onto the pair [`adi_marketplace::bundle::update`] takes — silently dropped
/// when it does not parse, so a stray `--force` value forces nothing rather than refusing the
/// whole update. Never meaningful for the project scaffold: nothing relands it in place regardless
/// of `force` (`docs/marketplace-bundles.md`'s "Update" section), so there is no pair to name it
/// with.
fn force_pair(raw: &str) -> Option<(Kind, &str)> {
    let (kind, name) = raw.split_once('/')?;
    Some((Kind::from_dir(kind)?, name))
}

/// One element's own update outcome, as a phrase.
fn element_update_line(e: &adi_marketplace::ElementUpdateOutcome) -> String {
    match (&e.note, e.changed) {
        (Some(note), _) => format!("{}/{}: {note}", e.kind, e.name),
        (None, true) => format!("{}/{} → {}", e.kind, e.name, e.id),
        (None, false) => format!("{}/{} unchanged", e.kind, e.name),
    }
}

/// One gallery entry, onto the wire type. The kind has already been decided by the store, so this
/// is the one place the two spellings of it meet.
fn media(m: adi_marketplace::AppMedia) -> MarketplaceMedia {
    MarketplaceMedia {
        url: m.url,
        kind: match m.kind {
            adi_marketplace::MediaKind::Video => MarketplaceMediaKind::Video,
            adi_marketplace::MediaKind::Image => MarketplaceMediaKind::Image,
        },
        caption: m.caption,
        poster: m.poster,
    }
}

/// Map a marketplace refusal onto its HTTP shape: work that would be lost is a 409 (the page
/// offers `force`), an unknown name is a 404, a manifest or a repository that does not belong is
/// a 502 — it is somebody else's server that is wrong — and everything else about the ask was
/// malformed.
fn refusal(e: &adi_marketplace::Error) -> Response {
    use adi_marketplace::Error as E;
    let status = match e {
        E::UnknownSource(_)
        | E::UnknownApp { .. }
        | E::NotInstalled(_)
        | E::UnknownElement(_)
        | E::BundleNotInstalled(_, _)
        | E::ElementNotInstalled(_) => 404,
        E::NotSynced(_) | E::Duplicate(_) | E::Dirty(_) | E::EmbeddingInUse(_) => 409,
        // Everything about the ask itself was malformed — including a per-kind store's own
        // refusal (`E::Store`), carried verbatim: every reason it can fire (a hive.yaml that is
        // not a mapping, a service already parked under this key, a project rename with no API to
        // call) is this ask's own fault, not somebody else's server.
        E::InvalidName(_)
        | E::NotHttps(_)
        | E::BadSpec(_)
        | E::BadAddress(_)
        | E::EmptyName
        | E::Store(_) => 400,
        // Everything a publisher got wrong, and everything git or the network refused: the ask
        // was fine, the far end was not.
        E::BadSlug(_)
        | E::BadRepo(_)
        | E::BadCommit { .. }
        | E::BadIcon { .. }
        | E::BadMedia { .. }
        | E::BadElementKind(_)
        | E::BadElementName { .. }
        | E::NotAnApp { .. }
        | E::CarriesRust { .. }
        | E::EmptyBundle(_)
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
        assert_eq!(status(&E::BadAddress("adi/crm/nope".into())), 400);
        assert_eq!(status(&E::BadElementKind("crm-suite".into())), 502);
        assert_eq!(status(&E::BadElementName("crm-suite".into(), "../evil".into())), 502);
        assert_eq!(
            status(&E::CarriesRust("crm-suite".into(), "a .rs file".into(), "tools/x.rs".into())),
            502
        );
        assert_eq!(status(&E::UnknownElement("adi/crm-suite/agents/nope".into())), 404);
        assert_eq!(
            status(&E::BundleNotInstalled("adi".into(), "crm-suite".into())),
            404
        );
        assert_eq!(
            status(&E::ElementNotInstalled("adi/crm-suite/agents/nope".into())),
            404
        );
        assert_eq!(status(&E::Store("not a mapping".into())), 400);
        assert_eq!(status(&E::EmbeddingInUse("e5 is still assigned".into())), 409);
    }

    // MARK: the general bundle path — installing, uninstalling, updating and starting an element

    /// A general bundle repository: an agent, a tool and a parked service, no dashboard at all.
    /// Answers its `file://` URL and its one commit.
    fn bundle_upstream(root: &Path) -> (String, String) {
        let dir = root.join("upstream");
        std::fs::create_dir_all(dir.join("agents")).expect("agents");
        std::fs::create_dir_all(dir.join("tools")).expect("tools");
        std::fs::create_dir_all(dir.join("services")).expect("services");
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
        std::fs::write(
            dir.join("agents").join("sales-bot.toml"),
            "backend = \"harness:adi\"\n",
        )
        .expect("agent");
        std::fs::write(dir.join("tools").join("csv-import.sh"), "#!/bin/sh\necho hi\n").expect("tool");
        std::fs::write(
            dir.join("services").join("redis.yaml"),
            "proxy:\n  host: redis.adi\n",
        )
        .expect("service");
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

    /// A source added and its cache seeded with a one-bundle manifest, previewing all three
    /// elements, pinning the fixture repo.
    fn seeded_bundle(tag: &str) -> (Config, String) {
        let cfg = store(tag);
        let (repo, commit) = bundle_upstream(&cfg.root().join("publisher"));
        adi_marketplace::sources::add(&cfg, "adi", "https://example/m.json").expect("add");
        cfg.module("marketplace")
            .write_raw(
                "cache/adi.json",
                format!(
                    "{{\"url\":\"https://example/m.json\",\"fetched_at\":1788288916,\
                     \"manifest\":{{\"name\":\"ADI starter apps\",\"bundles\":[\
                       {{\"slug\":\"crm-suite\",\"name\":\"CRM suite\",\
                        \"repo\":{repo:?},\"commit\":{commit:?},\"elements\":[\
                          {{\"kind\":\"agent\",\"name\":\"sales-bot\"}},\
                          {{\"kind\":\"tool\",\"name\":\"csv-import\"}},\
                          {{\"kind\":\"service\",\"name\":\"redis\"}}]}}]}}}}"
                )
                .as_bytes(),
            )
            .expect("cache");
        (cfg, commit)
    }

    #[test]
    fn the_listing_carries_a_bundles_preview_and_live_status() {
        let (cfg, _commit) = seeded_bundle("listing-status");
        let v: serde_json::Value = serde_json::from_str(&marketplace(&cfg).body).expect("json");
        assert_eq!(v["apps"][0]["elements"].as_array().map(Vec::len), Some(3));
        assert!(v["apps"][0]["bundle"].is_null(), "nothing installed yet");

        let _ = install_marketplace_app(
            &cfg,
            br#"{"marketplace":"adi","slug":"crm-suite","element":"agents/sales-bot"}"#,
        );
        let v: serde_json::Value = serde_json::from_str(&marketplace(&cfg).body).expect("json");
        let bundle = &v["apps"][0]["bundle"];
        assert_eq!(bundle["declared"], 3);
        assert_eq!(bundle["installed"].as_array().map(Vec::len), Some(1));
        assert!(!bundle["outdated"].as_bool().unwrap());
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn installing_a_single_element_lands_only_that_element() {
        let (cfg, _commit) = seeded_bundle("single-element");
        let res = install_marketplace_app(
            &cfg,
            br#"{"marketplace":"adi","slug":"crm-suite","element":"agents/sales-bot"}"#,
        );
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(done.message.contains("agents/sales-bot → sales-bot"), "{}", done.message);
        assert!(
            adi_agents::Agents::with_config(cfg.clone())
                .get("sales-bot")
                .expect("get")
                .is_some()
        );
        assert!(
            adi_tools::Tools::with_config(cfg.clone()).get("csv-import").expect("get").is_none(),
            "only the agent was asked for"
        );
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn uninstalling_an_element_drops_it_and_leaves_siblings() {
        let (cfg, _commit) = seeded_bundle("uninstall");
        let _ = install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"crm-suite"}"#);

        let res = uninstall_marketplace_element(
            &cfg,
            br#"{"marketplace":"adi","slug":"crm-suite","element":"agents/sales-bot"}"#,
        );
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(done.message.contains("uninstalled agents/sales-bot"), "{}", done.message);
        assert!(
            adi_agents::Agents::with_config(cfg.clone()).get("sales-bot").expect("get").is_none()
        );
        assert!(
            adi_tools::Tools::with_config(cfg.clone()).get("csv-import").expect("get").is_some(),
            "the sibling tool is untouched"
        );

        assert_eq!(
            uninstall_marketplace_element(
                &cfg,
                br#"{"marketplace":"adi","slug":"crm-suite","element":"agents/sales-bot"}"#,
            )
            .status,
            404,
            "already gone"
        );
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn starting_a_parked_service_lands_it_in_the_global_hive() {
        let (cfg, _commit) = seeded_bundle("start-service");
        let _ = install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"crm-suite"}"#);

        let res = start_marketplace_service(
            &cfg,
            br#"{"marketplace":"adi","slug":"crm-suite","name":"redis"}"#,
        );
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(done.message.contains("started redis"), "{}", done.message);
        let hive = cfg.module("hive").raw_path("hive.yaml");
        assert!(
            std::fs::read_to_string(&hive).expect("hive").contains("redis.adi"),
            "the service's own block landed in the global hive.yaml"
        );

        assert_eq!(
            start_marketplace_service(
                &cfg,
                br#"{"marketplace":"adi","slug":"crm-suite","name":"nope"}"#,
            )
            .status,
            404
        );
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn bundle_update_answers_even_when_there_is_nothing_to_do_and_refuses_a_stranger() {
        let (cfg, commit) = seeded_bundle("bundle-update");
        let _ = install_marketplace_app(&cfg, br#"{"marketplace":"adi","slug":"crm-suite"}"#);

        let res = update_marketplace_bundle(
            &cfg,
            br#"{"marketplace":"adi","slug":"crm-suite","force":[]}"#,
        );
        assert_eq!(res.status, 200, "{}", res.body);
        let done: MarketplaceDone = serde_json::from_str(&res.body).expect("done");
        assert!(done.message.contains(&commit[..7]), "{}", done.message);

        assert_eq!(
            update_marketplace_bundle(&cfg, br#"{"marketplace":"adi","slug":"ghost","force":[]}"#)
                .status,
            404
        );
        let _ = std::fs::remove_dir_all(cfg.root());
    }
}
