//! Install — land an app's bundle in the dashboard store without starting it — and start, the
//! one deliberate act that lets it run.
//!
//! An install is the panel's machine-to-machine import with two differences the marketplace owes
//! its reader:
//!
//! * **The slug is the entry's, not the bundle's.** A marketplace curates what it lists; the
//!   directory an app lands as is the slug the person typed, so `<marketplace>/<slug>` is the
//!   whole address of the thing they asked for.
//! * **It arrives inert.** The hive file is written under the parked name the supervisor's glob
//!   does not match, and `archived_at` is stamped — exactly the state Archive leaves a dashboard
//!   in, which the store already knows how to hold, list, and bring back. Nothing of somebody
//!   else's executes until `start` (or Restore on the Dashboards page) is a choice somebody
//!   made.
//!
//! A slug collision is refused, never numbered and never overwritten: a silent `crm-2` is a
//! surprise, and a silent replace is worse. `--force` replaces the files in place, keeping
//! whatever run state the app already has — the same mirror semantics a re-transfer has.

use serde::Serialize;
use std::path::Path;

use adi_dashboards::{
    DashboardBundle, HIVE_ARCHIVED, HIVE_LIVE, MAX_BUNDLE_FILES, Manifest, declared_host,
    decode_bundle, hive_yaml, preferred_host, read_manifest, write_import, write_manifest,
};

use crate::Marketplace;
use crate::cache;
use crate::error::{Error, Result};
use crate::fetch;
use crate::sources;
use adi_config::Config;

/// One app as a listing shows it: the entry's text, and where it stands on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CachedApp {
    /// The source's local name — the first half of `<marketplace>/<slug>`.
    pub marketplace: String,
    /// The entry's slug — the second half, and the directory an install lands as.
    pub slug: String,
    /// The entry's human name.
    pub name: String,
    /// The entry's one-liner.
    pub description: Option<String>,
    /// The entry's version, as published.
    pub version: Option<String>,
    /// Whether a dashboard directory by this slug is already on this machine.
    pub installed: bool,
    /// Whether that directory carries a live hive file — i.e. something is running it.
    pub started: bool,
    /// The hostname the started app answers on, when it declares one.
    pub host: Option<String>,
}

/// What an install answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Installed {
    /// The slug installed under — also the directory.
    pub slug: String,
    /// The name the bundle carries (what the Dashboards page will show).
    pub name: String,
    /// The hostname the app will answer on once started.
    pub host: String,
    /// Whether the install found the app already running and left it running (a `--force` over a
    /// started app; everything else lands inert).
    pub started: bool,
}

/// What a start answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Started {
    /// The slug started.
    pub slug: String,
    /// The hostname both of its services now claim — `<label>.adi`.
    pub host: String,
}

/// Every cached entry across every source, in source order — installed flags read live from the
/// dashboard store, so the listing can never disagree with the directory.
#[must_use]
pub fn cached_apps(config: &Config) -> Vec<CachedApp> {
    let dashboards = config.module("dashboards").dir().to_path_buf();
    let mut apps = Vec::new();
    for source in sources::list(config).unwrap_or_default() {
        let Some(envelope) = cache::read(config, &source.name) else {
            continue;
        };
        if envelope.url != source.url {
            continue;
        }
        let Some(manifest) = envelope.manifest else {
            continue;
        };
        for entry in manifest.apps {
            let dir = dashboards.join(&entry.slug);
            let started = dir.join(".adi").join(HIVE_LIVE).is_file();
            apps.push(CachedApp {
                marketplace: source.name.clone(),
                slug: entry.slug,
                name: entry.name,
                description: entry.description,
                version: entry.version,
                installed: dir.is_dir(),
                started,
                host: started.then(|| declared_host(&dir)).flatten(),
            });
        }
    }
    apps
}

/// Install `<marketplace>/<slug>` from the cache, fetching its artifact over HTTPS.
///
/// # Errors
/// Every way an install can be refused or fail: a malformed spec, an unknown source or app, no
/// cache, a slug collision (see [`Error::Collision`]), an artifact that is not a valid bundle,
/// or a write failure. Nothing is written on a refusal.
pub fn install(market: &Marketplace, spec: &str, force: bool) -> Result<Installed> {
    install_with(market, spec, force, fetch::get)
}

/// Install through a caller-supplied fetch — the seam the tests drive.
///
/// # Errors
/// As [`install`].
pub fn install_with(
    market: &Marketplace,
    spec: &str,
    force: bool,
    fetch: impl Fn(&str) -> std::result::Result<Vec<u8>, String>,
) -> Result<Installed> {
    let config = market.config();
    let (source_name, slug) = spec
        .split_once('/')
        .ok_or_else(|| Error::BadSpec(spec.to_string()))?;
    let source = sources::list(config)?
        .into_iter()
        .find(|s| s.name == source_name)
        .ok_or_else(|| Error::UnknownSource(source_name.to_string()))?;

    // From the cache, never the network: install works offline once a manifest is synced, and the
    // entry a person read on the listing is the entry they get.
    let envelope = cache::read(config, &source.name)
        .filter(|e| e.url == source.url)
        .ok_or_else(|| Error::NotSynced(source.name.clone()))?;
    let manifest = envelope
        .manifest
        .ok_or_else(|| Error::NotSynced(source.name.clone()))?;
    let entry = manifest.app(slug).ok_or_else(|| {
        Error::UnknownApp(source.name.clone(), slug.to_string(), manifest.slugs())
    })?;
    if !adi_config::valid_name(entry.slug.trim()) {
        return Err(Error::BadSlug(entry.slug.clone()));
    }

    let dashboards = config.module("dashboards");
    let dir = dashboards.dir().join(&entry.slug);
    // Refuse rather than number or overwrite — and count an id another dashboard was renamed
    // *from* as taken too, or the install would land where an old reference still points.
    let aliases = adi_config::Aliases::load(&dashboards).unwrap_or_default();
    if (!force && dir.exists()) || aliases.is_alias(&entry.slug) {
        return Err(Error::Collision(
            entry.slug.clone(),
            dir.display().to_string(),
        ));
    }

    let bundle = fetch_and_parse(&entry.slug, &entry.artifact, &fetch)?;
    let decoded = decode_bundle(&dir, &bundle.files)
        .map_err(|e| Error::BadArtifact(entry.slug.clone(), e.to_string()))?;

    // Capture the run state before the mirror: `.adi` survives it, so this is both what the app
    // had and what it keeps.
    let was_live = dir.join(".adi").join(HIVE_LIVE).is_file();
    write_import(&dir, &decoded)?;

    let name = bundle.name.trim().to_string();
    write_manifest(
        &dir,
        &Manifest {
            name: Some(name.clone()),
            description: adi_config::clean(bundle.description.as_deref()),
            // A marketplace app has no project on this machine; file it from the Dashboards
            // page, the way any other dashboard is filed.
            project: None,
            // The arrival state: archived until started. A `--force` over an app that was
            // already running stays running instead.
            archived_at: (!was_live).then(adi_config::now_unix),
            moved_to: None,
        },
    )?;

    let host = preferred_host(&dir, &name, bundle.host.as_deref());
    let (live, parked) = if was_live {
        (HIVE_LIVE, Some(HIVE_ARCHIVED))
    } else {
        (HIVE_ARCHIVED, Some(HIVE_LIVE))
    };
    if let Err(e) = std::fs::create_dir_all(dir.join(".adi"))
        .and_then(|()| std::fs::write(dir.join(".adi").join(live), hive_yaml(&dir, &host)))
    {
        return Err(Error::Io(e));
    }
    // Whichever of the two names the install did not write must not linger: two hive files would
    // describe one dashboard, and the supervisor's glob could pick the parked one's stale host.
    if let Some(remove) = parked {
        let _ = std::fs::remove_file(dir.join(".adi").join(remove));
    }

    Ok(Installed {
        slug: entry.slug.clone(),
        name,
        host,
        started: was_live,
    })
}

/// Fetch an entry's artifact and parse it into a bundle this build will land.
fn fetch_and_parse(
    slug: &str,
    artifact: &str,
    fetch: &impl Fn(&str) -> std::result::Result<Vec<u8>, String>,
) -> Result<DashboardBundle> {
    let bytes = fetch(artifact).map_err(Error::Fetch)?;
    let bundle: DashboardBundle = serde_json::from_slice(&bytes)
        .map_err(|e| Error::BadArtifact(slug.to_string(), format!("not valid bundle JSON: {e}")))?;
    if bundle.name.trim().is_empty() {
        return Err(Error::BadArtifact(
            slug.to_string(),
            "the bundle names no dashboard".to_string(),
        ));
    }
    if bundle.files.is_empty() {
        return Err(Error::BadArtifact(
            slug.to_string(),
            "the bundle carries no files".to_string(),
        ));
    }
    if bundle.files.len() > MAX_BUNDLE_FILES {
        return Err(Error::BadArtifact(
            slug.to_string(),
            "the bundle carries too many files".to_string(),
        ));
    }
    Ok(bundle)
}

/// Start an installed app: move its hive file into the supervisor's glob, so both bun servers
/// come up on leased ports within a few seconds. Accepts `<marketplace>/<slug>` or the bare
/// `<slug>` — by the time anything is started, the dashboard store is the whole address.
///
/// Idempotent: starting an app that is already started answers with the host it answers on.
///
/// # Errors
/// [`Error::NotInstalled`] when no dashboard directory by that slug exists, plus write failures.
pub fn start(market: &Marketplace, spec_or_slug: &str) -> Result<Started> {
    let config = market.config();
    let slug = spec_or_slug
        .rsplit('/')
        .next()
        .unwrap_or(spec_or_slug)
        .trim();
    let dir = config.module("dashboards").dir().join(slug);
    if !slug.is_empty() && !dir.is_dir() {
        return Err(Error::NotInstalled(slug.to_string()));
    }

    let mut manifest = read_manifest(&dir);
    let name = manifest.name.clone().unwrap_or_else(|| slug.to_string());
    // The parked file's own host is the preference — a label chosen when the app arrived — but
    // only a preference: if a neighbour has since taken it, a fresh one is derived rather than
    // handing two dashboards one hostname.
    let host = preferred_host(&dir, &name, declared_host(&dir).as_deref());
    std::fs::create_dir_all(dir.join(".adi"))
        .and_then(|()| std::fs::write(dir.join(".adi").join(HIVE_LIVE), hive_yaml(&dir, &host)))
        .map_err(Error::Io)?;
    // Two hive files would describe one dashboard; the live one is the whole truth now.
    let _ = std::fs::remove_file(dir.join(".adi").join(HIVE_ARCHIVED));

    manifest.archived_at = None;
    // This machine runs it again, so it does not live somewhere else.
    manifest.moved_to = None;
    write_manifest(&dir, &manifest).map_err(Error::Io)?;

    Ok(Started {
        slug: slug.to_string(),
        host,
    })
}

/// Whether `dir` holds a live hive file — the one file whose presence says something runs this.
#[must_use]
pub fn is_started(dir: &Path) -> bool {
    dir.join(".adi").join(HIVE_LIVE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_dashboards::BundleFile;
    use base64::Engine as _;
    use std::path::PathBuf;

    /// The marketplace a fixture stands in for: one source, one manifest, one app.
    const MANIFEST: &str = r#"{"name":"ADI starter apps","apps":[
        {"slug":"crm","name":"CRM","version":"0.1.0",
         "description":"Who has gone quiet, and what was last said to them.",
         "artifact":"https://example/artifacts/crm.bundle.json"}
    ]}"#;

    /// The artifact the fixture URL "hosts": a CRM-shaped dashboard bundle, packed the way the
    /// panel's export packs one — authored files only, no manifest, no `.adi`.
    fn crm_bundle(panel: &str, route: &str) -> Vec<u8> {
        let b64 = |text: &str| base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let files = [
            ("frontend/index.ts", "// the frontend entry\n"),
            ("frontend/index.html", "<!doctype html>\n"),
            ("frontend/modules/contacts.ts", panel),
            ("backend/index.ts", "// the backend entry\n"),
            ("backend/routes/contacts.ts", route),
            ("README.md", "# CRM\n"),
        ];
        let bundle = DashboardBundle {
            id: "whatever-the-publisher-had".to_string(),
            name: "CRM".to_string(),
            description: Some("Who has gone quiet, and what was last said to them.".to_string()),
            project: Some("a-project-only-where-it-was-built".to_string()),
            host: Some("crm.adi".to_string()),
            files: files
                .into_iter()
                .map(|(path, text)| BundleFile {
                    path: path.to_string(),
                    contents: b64(text),
                })
                .collect(),
        };
        serde_json::to_vec(&bundle).expect("a bundle serializes")
    }

    /// A synced store with the fixture source cached, and a fetch that answers for the artifact
    /// URL only.
    fn synced(tag: &str, panel: &str, route: &str) -> Marketplace {
        let market = crate::tests::scratch(tag);
        sources::add(market.config(), "adi", "https://example/marketplace.json").expect("add");
        crate::sync::sync_with(&market, |url| {
            if url == "https://example/marketplace.json" {
                Ok(MANIFEST.as_bytes().to_vec())
            } else if url == "https://example/artifacts/crm.bundle.json" {
                Ok(crm_bundle(panel, route))
            } else {
                Err(format!("no such fixture: {url}"))
            }
        })
        .expect("sync");
        market
    }

    /// The installed CRM's directory.
    fn crm_dir(market: &Marketplace) -> PathBuf {
        market.dashboards_dir().join("crm")
    }

    #[test]
    fn an_install_lands_the_files_and_starts_nothing() {
        let market = synced(
            "inert",
            "export default () => 1;\n",
            "export default () => 2;\n",
        );
        let done = install_with(&market, "adi/crm", false, |_url: &str| {
            Ok(crm_bundle(
                "export default () => 1;\n",
                "export default () => 2;\n",
            ))
        })
        .unwrap_or_else(|e| panic!("{e}"));
        let dir = crm_dir(&market);

        // The app's files arrived, under the entry's slug.
        assert_eq!(done.slug, "crm");
        assert_eq!(done.name, "CRM");
        assert_eq!(done.host, "crm.adi", "the offered label was free here");
        assert!(!done.started, "an install never starts anything");
        assert_eq!(
            std::fs::read_to_string(dir.join("frontend").join("modules").join("contacts.ts"))
                .expect("panel"),
            "export default () => 1;\n"
        );

        // Inert on arrival: no live hive file, the parked one instead, and the manifest says
        // archived — the state the store already knows how to hold and Restore.
        assert!(!is_started(&dir), "the supervisor's glob must not match");
        assert!(dir.join(".adi").join(HIVE_ARCHIVED).is_file());
        let manifest = read_manifest(&dir);
        assert!(manifest.archived_at.is_some(), "arrival is archived");
        assert_eq!(manifest.name.as_deref(), Some("CRM"));
        assert_eq!(
            manifest.project, None,
            "a project id from where it was built means nothing here"
        );
        let hive = std::fs::read_to_string(dir.join(".adi").join(HIVE_ARCHIVED)).expect("hive");
        assert!(hive.contains("host: crm.adi"), "{hive}");
        assert!(
            hive.contains(&format!("working_dir: {}", dir.display())),
            "{hive}"
        );

        // And the listing reports all three states honestly.
        let apps = cached_apps(market.config());
        assert_eq!(apps.len(), 1);
        assert_eq!(
            (
                &apps[0].installed,
                &apps[0].started,
                apps[0].host.as_deref()
            ),
            (&true, &false, None),
            "{apps:?}"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_slug_collision_is_refused_and_names_the_way_past_it() {
        let market = synced("collision", "x", "y");
        // Something already answers to `crm` — a dashboard built here, say.
        let dir = crm_dir(&market);
        std::fs::create_dir_all(dir.join("frontend")).expect("seed");
        std::fs::write(dir.join("frontend").join("index.ts"), "mine").expect("seed");

        let err = install_with(&market, "adi/crm", false, |_| unreachable!()).expect_err("refused");
        assert!(
            matches!(&err, Error::Collision(slug, _) if slug == "crm"),
            "{err}"
        );
        assert!(err.to_string().contains("--force"), "{err}");
        // Refused means untouched.
        assert_eq!(
            std::fs::read_to_string(dir.join("frontend").join("index.ts")).expect("seed"),
            "mine"
        );

        // Force replaces the files — a mirror, keeping `.adi` and `node_modules` like any
        // re-transfer — and lands just as inert as a first install.
        std::fs::create_dir_all(dir.join("node_modules")).expect("cache dir");
        std::fs::write(dir.join("node_modules").join("dep.js"), "cached").expect("cache");
        let done =
            install_with(&market, "adi/crm", true, |_| Ok(crm_bundle("v2", "v2"))).expect("force");
        assert!(!done.started);
        assert_eq!(
            std::fs::read_to_string(dir.join("frontend").join("modules").join("contacts.ts"))
                .expect("replaced"),
            "v2"
        );
        assert!(
            std::fs::read_to_string(dir.join("node_modules").join("dep.js")).is_ok(),
            "the machine's own cache survives a reinstall"
        );
        assert!(!is_started(&dir));
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_force_over_a_started_app_keeps_it_running() {
        let market = synced("redeploy", "v1", "v1");
        let v1 = |_: &str| Ok(crm_bundle("v1", "v1"));
        let v2 = |_: &str| Ok(crm_bundle("v2", "v2"));
        install_with(&market, "adi/crm", false, v1).expect("install");
        start(&market, "crm").expect("start");
        let dir = crm_dir(&market);

        let done = install_with(&market, "adi/crm", true, v2).expect("force");
        assert!(
            done.started,
            "a running app keeps running through a reinstall"
        );
        assert!(is_started(&dir), "the live hive file stands");
        assert!(!dir.join(".adi").join(HIVE_ARCHIVED).exists());
        assert!(
            read_manifest(&dir).archived_at.is_none(),
            "still live, not archived"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("frontend").join("modules").join("contacts.ts"))
                .expect("updated"),
            "v2"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn start_moves_the_hive_file_into_the_glob_and_cleans_up_after_itself() {
        let market = synced("start", "v1", "v1");
        install_with(&market, "adi/crm", false, |_| Ok(crm_bundle("v1", "v1"))).expect("install");
        let dir = crm_dir(&market);

        let started = start(&market, "adi/crm").expect("start");
        assert_eq!(started.host, "crm.adi");
        assert!(is_started(&dir));
        assert!(!dir.join(".adi").join(HIVE_ARCHIVED).exists());
        assert!(read_manifest(&dir).archived_at.is_none());

        // The listing flips with it.
        let apps = cached_apps(market.config());
        assert_eq!(
            (apps[0].installed, apps[0].started, apps[0].host.as_deref()),
            (true, true, Some("crm.adi")),
            "{apps:?}"
        );

        // Idempotent, and a bare slug answers too.
        let again = start(&market, "crm").expect("start again");
        assert_eq!(again.host, "crm.adi");

        // Nothing installed, nothing to start.
        assert!(matches!(
            start(&market, "ghost"),
            Err(Error::NotInstalled(_))
        ));
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn start_gives_way_when_the_arrival_host_was_taken_meanwhile() {
        let market = synced("taken", "v1", "v1");
        install_with(&market, "adi/crm", false, |_| Ok(crm_bundle("v1", "v1"))).expect("install");

        // A neighbour claims `crm.adi` after the app arrived.
        let neighbour = market.dashboards_dir().join("resident-crm");
        std::fs::create_dir_all(neighbour.join(".adi")).expect("neighbour");
        std::fs::write(
            neighbour.join(".adi").join(HIVE_LIVE),
            "version: \"1\"\nservices:\n  frontend:\n    proxy:\n      host: crm.adi\n",
        )
        .expect("hive");

        let started = start(&market, "crm").expect("start");
        // The parked file's label is only a preference: a neighbour has since taken `crm.adi`,
        // and two dashboards on one hostname is a routing coin-flip — so a fresh label is
        // derived rather than handed out, exactly as an arriving transfer's is.
        assert_eq!(started.host, "dashboard.adi", "{}", started.host);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn every_wrong_ask_is_refused_with_the_sentence_that_names_it() {
        let market = synced("refusals", "v1", "v1");

        // A spec that names no marketplace.
        assert!(matches!(
            install_with(&market, "crm", false, |_| unreachable!()),
            Err(Error::BadSpec(_))
        ));
        // An unknown marketplace, and an unknown app in a known one.
        assert!(matches!(
            install_with(&market, "other/crm", false, |_| unreachable!()),
            Err(Error::UnknownSource(_))
        ));
        assert!(matches!(
            install_with(&market, "adi/nope", false, |_| unreachable!()),
            Err(Error::UnknownApp(_, slug, slugs)) if slug == "nope" && slugs.contains("crm")
        ));

        // No cache: a source never synced, and one whose cache is for another URL.
        let bare = crate::tests::scratch("refusals-bare");
        sources::add(bare.config(), "adi", "https://example/m.json").expect("add");
        assert!(matches!(
            install_with(&bare, "adi/crm", false, |_| unreachable!()),
            Err(Error::NotSynced(_))
        ));
        sources::add(bare.config(), "moved", "https://example/other.json").expect("add");
        bare.config()
            .module(crate::MODULE)
            .write_raw(
                "cache/moved.json",
                br#"{"url":"https://the-old-url.example/m.json","manifest":{"apps":[]}}"#,
            )
            .expect("a cache for another url");
        assert!(matches!(
            install_with(&bare, "moved/crm", false, |_| unreachable!()),
            Err(Error::NotSynced(_))
        ));
        let _ = std::fs::remove_dir_all(bare.config().root());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_artifact_that_does_not_belong_is_refused_with_nothing_written() {
        let market = synced("bad-artifact", "v1", "v1");

        // A bundle trying to choose this machine's routing.
        let escape = DashboardBundle {
            id: "x".to_string(),
            name: "CRM".to_string(),
            description: None,
            project: None,
            host: None,
            files: vec![BundleFile {
                path: ".adi/hive.yaml".to_string(),
                contents: String::new(),
            }],
        };
        let bytes = serde_json::to_vec(&escape).expect("serialize");
        let err =
            install_with(&market, "adi/crm", false, |_| Ok(bytes.clone())).expect_err("refused");
        assert!(matches!(err, Error::BadArtifact(_, _)), "{err}");
        assert!(
            !crm_dir(&market).exists(),
            "a refused install left a directory behind"
        );

        // Not a bundle at all.
        let err = install_with(&market, "adi/crm", false, |_| Ok(b"<html>".to_vec()))
            .expect_err("refused");
        assert!(matches!(err, Error::BadArtifact(_, _)), "{err}");
        // The fetch itself failing.
        assert!(matches!(
            install_with(&market, "adi/crm", false, |_| Err("502".to_string())),
            Err(Error::Fetch(_))
        ));
        assert!(!crm_dir(&market).exists());
        let _ = std::fs::remove_dir_all(market.config().root());
    }
}
