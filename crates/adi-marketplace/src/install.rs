//! Install — clone an app's repository at the commit its manifest pins — and the two acts that
//! follow it: start, which is what lets somebody else's code run, and update, which moves the
//! clone onto a newer pin.
//!
//! Three things about an install are deliberate, and each one is the answer to how the first
//! version of this got it wrong:
//!
//! * **The operator names their copy.** The entry's slug is the *published* identity; the
//!   directory, the id and the hostname all come from the name the person typed at install time
//!   (renameable afterwards like any dashboard's). Two copies of one app are therefore ordinary,
//!   and nothing is ever refused for a name collision — the store mints `crm-2` the way it does
//!   everywhere else.
//! * **What lands is a clone, not a copy.** `.git` is kept and the pinned commit sits on a branch
//!   that tracks `origin`, so the app can be edited, committed to, and pulled — which is the whole
//!   reason a marketplace app is a repository rather than a bundle of bytes.
//! * **It arrives inert.** The hive file is written under the parked name the supervisor's glob
//!   does not match, and `archived_at` is stamped. Nothing of somebody else's executes until
//!   [`start`] is a choice somebody made.
//!
//! The clone lands in a **staging directory outside the dashboards tree** and is moved in whole.
//! That is not tidiness: `dashboards/*/.adi/hive.yaml` is the supervisor's import glob, so a
//! repository that ships one of its own would have a few seconds in which its runner — any
//! script, any container — was live before this code could overwrite it. Nothing a clone carries
//! at `.adi/` or `hive.yaml` survives staging.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use adi_dashboards::{
    HIVE_ARCHIVED, HIVE_LIVE, Manifest, declared_host, hive_yaml, preferred_host, read_manifest,
    write_manifest,
};

use crate::Marketplace;
use crate::cache;
use crate::error::{Error, Result};
use crate::git;
use crate::manifest::AppEntry;
use crate::sources;
use adi_config::Config;

/// The file an installed app carries, inside the store-owned `.adi/`: which entry it came from,
/// and which commit it stands at. It is what makes "installed" a fact about a directory rather
/// than a guess from its name, and what an update reads to know where to go next.
const RECORD_FILE: &str = "marketplace.json";

/// The word a dashboard id falls back to when a name slugs to nothing (all punctuation, or a
/// script with no Latin in it) — the same fallback the panel's own create path uses.
const ID_FALLBACK: &str = "app";

/// The two files a repository must have at its root to be an app this can run. They are the
/// dashboard contract (`guides/dashboards.md`), and the hive file written here runs exactly
/// them — so a repository without them installs into something that could never start.
const REQUIRED_FILES: &[&str] = &["frontend/index.ts", "backend/index.ts"];

/// What a clone may never bring into the dashboards tree: the store's own directory, and a hive
/// file at the root. Both are read by the supervisor and the front door; neither is the
/// publisher's to write.
const STRIPPED_ON_ARRIVAL: &[&str] = &[".adi", "hive.yaml"];

/// The record an installed app carries at `.adi/marketplace.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    /// The source's local name — the first half of `<marketplace>/<slug>`.
    pub marketplace: String,
    /// The entry's published slug.
    pub slug: String,
    /// The repository the clone came from.
    pub repo: String,
    /// The commit this copy stands at. Compared against the manifest's pin to know whether an
    /// update is waiting.
    pub commit: String,
    /// The branch that commit sits on — what a `git pull` in this directory follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// When it was installed, in Unix seconds.
    pub installed_at: u64,
    /// When it was last moved onto a new pin, if it ever was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

/// One app as a listing shows it: what the manifest published, and every copy of it on this
/// machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CachedApp {
    /// The source's local name — the first half of `<marketplace>/<slug>`.
    pub marketplace: String,
    /// The entry's published slug — the second half, and how an install is addressed.
    pub slug: String,
    /// The entry's human name, offered as the default when somebody names their copy.
    pub name: String,
    /// The entry's one-liner.
    pub description: Option<String>,
    /// The entry's version, as published.
    pub version: Option<String>,
    /// The repository an install clones.
    pub repo: String,
    /// The commit the manifest pins right now.
    pub commit: String,
    /// The branch that commit sits on, when the entry names one.
    pub branch: Option<String>,
    /// Every copy of this app installed here — empty when there is none.
    pub installs: Vec<AppInstall>,
}

/// One installed copy of an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInstall {
    /// The dashboard id — the directory, and how start and update address it.
    pub id: String,
    /// The name the operator gave this copy.
    pub name: String,
    /// The commit this copy stands at.
    pub commit: String,
    /// Whether something is running it — its hive file is in the supervisor's glob.
    pub started: bool,
    /// The hostname it answers on (or will, once started).
    pub host: Option<String>,
    /// Whether the manifest now pins a different commit than this copy stands at.
    pub outdated: bool,
}

/// What an install answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Installed {
    /// The dashboard id the copy landed as — minted from the name, so it may be numbered.
    pub id: String,
    /// The name this copy carries.
    pub name: String,
    /// The hostname it answers on once started.
    pub host: String,
    /// The commit it stands at.
    pub commit: String,
    /// Whether it was started as part of the install.
    pub started: bool,
    /// Anything true about the install that the operator would rather hear now than find out
    /// later — a repository that versions the files the store owns, say.
    pub notes: Vec<String>,
}

/// What a start answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Started {
    /// The dashboard id started.
    pub id: String,
    /// The hostname both of its services now claim — `<label>.adi`.
    pub host: String,
}

/// What an update answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Updated {
    /// The dashboard id updated.
    pub id: String,
    /// The commit it stood at before.
    pub from: String,
    /// The commit it stands at now.
    pub to: String,
    /// Whether it moved at all — an update onto the pin it already had is not a failure.
    pub changed: bool,
    /// Whether it is running, and so has already hot-reloaded onto the new code.
    pub started: bool,
}

/// Read an installed app's record, or `None` for a dashboard that did not come from a
/// marketplace.
#[must_use]
pub fn read_record(dir: &Path) -> Option<InstallRecord> {
    let raw = std::fs::read(dir.join(".adi").join(RECORD_FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Write an installed app's record.
fn write_record(dir: &Path, record: &InstallRecord) -> Result<()> {
    std::fs::create_dir_all(dir.join(".adi"))?;
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| Error::Fetch(format!("encoding the install record: {e}")))?;
    std::fs::write(dir.join(".adi").join(RECORD_FILE), bytes)?;
    Ok(())
}

/// Every app installed from a marketplace, as `(id, directory, record)`.
#[must_use]
pub fn installed(config: &Config) -> Vec<(String, PathBuf, InstallRecord)> {
    let root = config.module("dashboards").dir().to_path_buf();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf, InstallRecord)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|dir| {
            let record = read_record(&dir)?;
            let id = dir.file_name()?.to_string_lossy().into_owned();
            Some((id, dir, record))
        })
        .collect();
    // Directory order is the filesystem's business; a listing has to be stable between reads.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Every cached entry across every source, in source order, each carrying the copies of it that
/// are installed here — read live from the dashboard store, so the listing can never disagree
/// with the directory.
#[must_use]
pub fn cached_apps(config: &Config) -> Vec<CachedApp> {
    let here = installed(config);
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
            let pin = entry.pin();
            let installs = here
                .iter()
                .filter(|(_, _, record)| {
                    record.marketplace == source.name && record.slug == entry.slug
                })
                .map(|(id, dir, record)| AppInstall {
                    id: id.clone(),
                    name: read_manifest(dir).name.unwrap_or_else(|| id.clone()),
                    commit: record.commit.clone(),
                    started: is_started(dir),
                    host: declared_host(dir),
                    outdated: record.commit.to_ascii_lowercase() != pin,
                })
                .collect();
            apps.push(CachedApp {
                marketplace: source.name.clone(),
                slug: entry.slug,
                name: entry.name,
                description: entry.description,
                version: entry.version,
                repo: entry.repo,
                commit: pin,
                branch: entry.branch,
                installs,
            });
        }
    }
    apps
}

/// Install `<marketplace>/<slug>` as a dashboard called `name`: clone the entry's repository at
/// the commit it pins, land it inert, and — only if asked — start it.
///
/// `name` is the operator's, and empty means "use the entry's own name". The id is minted from
/// it, so installing the same app twice gives a second copy rather than a refusal.
///
/// # Errors
/// Every way an install can be refused: a malformed spec, an unknown source or app, a source
/// never synced, an entry that does not validate, a clone or a pin that failed, a repository that
/// is not laid out as an app, or a write failure. Nothing is left in the dashboards tree on a
/// refusal.
pub fn install(market: &Marketplace, spec: &str, name: &str, start_it: bool) -> Result<Installed> {
    let config = market.config();
    let (source_name, slug) = spec
        .split_once('/')
        .ok_or_else(|| Error::BadSpec(spec.to_string()))?;
    let entry = entry_of(config, source_name, slug)?;
    entry.validate()?;

    let name = match name.trim() {
        "" => entry.name.trim(),
        given => given,
    };
    if name.is_empty() {
        return Err(Error::EmptyName);
    }

    let module = config.module("dashboards");
    // The id is a slug of the name, minted the way every other store id is: numbered past
    // anything already there, and past any id a dashboard was renamed *from*.
    let aliases = adi_config::Aliases::load(&module).unwrap_or_default();
    let id = adi_config::mint(name, ID_FALLBACK, |candidate| {
        module.dir().join(candidate).exists() || aliases.is_alias(candidate)
    });
    let dir = module.dir().join(&id);

    let staged_dir = staging_dir(market).join(&id);
    let (pin, notes) = stage(market, &id, &entry)?;

    // The move in. Everything before this happened outside the supervisor's glob.
    std::fs::create_dir_all(module.dir())?;
    if let Err(e) = std::fs::rename(&staged_dir, &dir) {
        let _ = std::fs::remove_dir_all(&staged_dir);
        return Err(Error::Io(e));
    }

    match land(&dir, source_name, name, &entry, &pin) {
        Ok(host) => {
            let started = if start_it {
                start(market, &id).is_ok()
            } else {
                false
            };
            Ok(Installed {
                id,
                name: name.to_string(),
                host,
                commit: pin.commit,
                started,
                notes,
            })
        }
        Err(e) => {
            // A half-landed app is a directory the supervisor and the Dashboards page would both
            // pick up as a broken one; there is nothing here worth keeping.
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// The directory clones are assembled in — under the marketplace module, deliberately *not*
/// under `dashboards/`, so nothing half-built is ever inside the supervisor's import glob.
fn staging_dir(market: &Marketplace) -> PathBuf {
    market.config().module(crate::MODULE).dir().join("staging")
}

/// Clone the entry into staging, strip what a repository may not bring, and say what stood out.
/// The staged directory is left at `staging/<id>` for the caller to move in.
fn stage(market: &Marketplace, id: &str, entry: &AppEntry) -> Result<(git::Pin, Vec<String>)> {
    let staging = staging_dir(market);
    let dest = staging.join(id);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&staging)?;

    let outcome = (|| -> Result<(git::Pin, Vec<String>)> {
        let pin = git::clone_pinned(&entry.repo, &entry.pin(), entry.branch(), &dest)
            .map_err(Error::Git)?;

        let missing: Vec<&str> = REQUIRED_FILES
            .iter()
            .copied()
            .filter(|f| !dest.join(f).is_file())
            .collect();
        if !missing.is_empty() {
            return Err(Error::NotAnApp(
                entry.slug.clone(),
                format!("it has no {}", missing.join(" and no ")),
            ));
        }

        let mut notes = Vec::new();
        for name in STRIPPED_ON_ARRIVAL {
            let path = dest.join(name);
            if !path.exists() {
                continue;
            }
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
            notes.push(format!(
                "the repository ships a {name} of its own — dropped, because the hive file and the \
                 host are this machine's to write"
            ));
        }
        // Written before anything of the store's is: the point is that `git status` in this copy
        // never shows the store's files as work the operator forgot to commit. The excludes cover
        // what the store *adds*; the skip covers what it *rewrites* — the generated entry points,
        // which the panel restamps in place whenever its templates move on.
        git::exclude_store_files(&dest)?;
        git::ignore_generated(&dest);
        if git::tracks(&dest, "config.toml") {
            notes.push(
                "the repository versions config.toml, which is this dashboard's own manifest — a \
                 later update will fight it over the name you chose"
                    .to_string(),
            );
        }
        Ok((pin, notes))
    })();

    if outcome.is_err() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    outcome
}

/// Write everything the store owns into a landed clone: the manifest, the install record, and the
/// parked hive file. Answers the hostname the app will claim.
///
/// The host is derived here rather than in staging because it depends on what the *neighbours*
/// claim, and a directory outside `dashboards/` has none.
fn land(
    dir: &Path,
    marketplace: &str,
    name: &str,
    entry: &AppEntry,
    pin: &git::Pin,
) -> Result<String> {
    write_manifest(
        dir,
        &Manifest {
            name: Some(name.to_string()),
            description: adi_config::clean(entry.description.as_deref()),
            // A marketplace app has no project on this machine; file it from the Dashboards page,
            // the way any other dashboard is filed.
            project: None,
            // The arrival state: archived until started.
            archived_at: Some(adi_config::now_unix()),
            moved_to: None,
        },
    )?;
    write_record(
        dir,
        &InstallRecord {
            marketplace: marketplace.to_string(),
            slug: entry.slug.clone(),
            repo: entry.repo.clone(),
            commit: pin.commit.clone(),
            branch: Some(pin.branch.clone()),
            installed_at: adi_config::now_unix(),
            updated_at: None,
        },
    )?;

    let host = preferred_host(dir, name, None);
    std::fs::create_dir_all(dir.join(".adi"))?;
    std::fs::write(dir.join(".adi").join(HIVE_ARCHIVED), hive_yaml(dir, &host))?;
    Ok(host)
}

/// The cached entry `<marketplace>/<slug>` names, from the cache and never the network — install
/// works offline once a manifest is synced, and the entry a person read on the listing is the
/// entry they get.
fn entry_of(config: &Config, source_name: &str, slug: &str) -> Result<AppEntry> {
    let source = sources::list(config)?
        .into_iter()
        .find(|s| s.name == source_name)
        .ok_or_else(|| Error::UnknownSource(source_name.to_string()))?;
    let envelope = cache::read(config, &source.name)
        .filter(|e| e.url == source.url)
        .ok_or_else(|| Error::NotSynced(source.name.clone()))?;
    let manifest = envelope
        .manifest
        .ok_or_else(|| Error::NotSynced(source.name.clone()))?;
    manifest
        .app(slug)
        .cloned()
        .ok_or_else(|| Error::UnknownApp(source.name.clone(), slug.to_string(), manifest.slugs()))
}

/// Start an installed app: move its hive file into the supervisor's glob, so both bun servers
/// come up on leased ports within a few seconds.
///
/// Idempotent: starting an app that is already started answers with the host it answers on.
///
/// # Errors
/// [`Error::NotInstalled`] when no dashboard directory by that id exists, plus write failures.
pub fn start(market: &Marketplace, id: &str) -> Result<Started> {
    let config = market.config();
    let id = id.trim();
    let dir = config.module("dashboards").dir().join(id);
    if id.is_empty() || !dir.is_dir() {
        return Err(Error::NotInstalled(id.to_string()));
    }

    let mut manifest = read_manifest(&dir);
    let name = manifest.name.clone().unwrap_or_else(|| id.to_string());
    // The parked file's own host is the preference — a label chosen when the app arrived — but
    // only a preference: if a neighbour has since taken it, a fresh one is derived rather than
    // handing two dashboards one hostname.
    let host = preferred_host(&dir, &name, declared_host(&dir).as_deref());
    std::fs::create_dir_all(dir.join(".adi"))?;
    std::fs::write(dir.join(".adi").join(HIVE_LIVE), hive_yaml(&dir, &host))?;
    // Two hive files would describe one dashboard; the live one is the whole truth now.
    let _ = std::fs::remove_file(dir.join(".adi").join(HIVE_ARCHIVED));

    manifest.archived_at = None;
    // This machine runs it again, so it does not live somewhere else.
    manifest.moved_to = None;
    write_manifest(&dir, &manifest)?;

    Ok(Started {
        id: id.to_string(),
        host,
    })
}

/// Move an installed app onto the commit its marketplace now pins.
///
/// A fast-forward, never a reset: an operator's own commits on top of an installed app are the
/// point of it being a clone. Uncommitted work stops the update outright, and `force` is the
/// deliberate second ask that resets onto the pin and loses it.
///
/// # Errors
/// [`Error::NotInstalled`] for an id that is not an installed app, [`Error::UnknownSource`] /
/// [`Error::NotSynced`] / [`Error::UnknownApp`] when the entry it came from is no longer listed,
/// [`Error::Dirty`] for uncommitted work, and [`Error::Git`] for anything git refused.
pub fn update(market: &Marketplace, id: &str, force: bool) -> Result<Updated> {
    let config = market.config();
    let id = id.trim();
    let dir = config.module("dashboards").dir().join(id);
    let Some(mut record) = (!id.is_empty()).then(|| read_record(&dir)).flatten() else {
        return Err(Error::NotInstalled(id.to_string()));
    };
    let entry = entry_of(config, &record.marketplace, &record.slug)?;
    entry.validate()?;

    let from = record.commit.to_ascii_lowercase();
    let to = entry.pin();
    if from == to && !force {
        return Ok(Updated {
            id: id.to_string(),
            from,
            to,
            changed: false,
            started: is_started(&dir),
        });
    }
    // The generated entry points come back under git's eye for the move — they are the pin's, not
    // the panel's, while a merge is walking the tree — and go back to being ignored after it.
    git::unignore_generated(&dir);
    if !force && git::dirty(&dir) {
        git::ignore_generated(&dir);
        return Err(Error::Dirty(id.to_string()));
    }

    let branch = entry.branch().or(record.branch.as_deref()).map(str::to_string);
    let pin = git::move_to(&dir, &to, branch.as_deref(), force);
    git::ignore_generated(&dir);
    let pin = pin.map_err(Error::Git)?;

    record.commit.clone_from(&pin.commit);
    record.branch = Some(pin.branch);
    record.repo.clone_from(&entry.repo);
    record.updated_at = Some(adi_config::now_unix());
    write_record(&dir, &record)?;

    Ok(Updated {
        id: id.to_string(),
        from,
        to: pin.commit,
        changed: true,
        started: is_started(&dir),
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

    /// A real repository standing in for what a publisher hosts: a dashboard-shaped app with two
    /// commits. Answers its `file://` URL and the two commits, oldest first.
    fn upstream(root: &Path, panel: &str) -> (String, String, String) {
        let dir = root.join("upstream");
        std::fs::create_dir_all(dir.join("frontend").join("modules")).expect("frontend");
        std::fs::create_dir_all(dir.join("backend").join("routes")).expect("backend");
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
        for (path, text) in [
            ("frontend/index.ts", "// the frontend entry\n"),
            ("frontend/index.html", "<!doctype html>\n"),
            ("backend/index.ts", "// the backend entry\n"),
            ("README.md", "# CRM\n"),
        ] {
            std::fs::write(dir.join(path), text).expect("seed");
        }
        std::fs::write(dir.join("frontend").join("modules").join("contacts.ts"), panel)
            .expect("panel");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "v1"]);
        let first = head_of(&dir);
        std::fs::write(dir.join("frontend").join("modules").join("contacts.ts"), "v2\n")
            .expect("panel v2");
        git(&["commit", "--quiet", "-am", "v2"]);
        (format!("file://{}", dir.display()), first, head_of(&dir))
    }

    /// The commit a repository's HEAD is on.
    fn head_of(dir: &Path) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A store with one source whose cache carries one entry, pinned at `commit`.
    fn synced(market: &Marketplace, repo: &str, commit: &str) {
        let manifest = format!(
            r#"{{"name":"ADI starter apps","apps":[
                 {{"slug":"crm","name":"CRM","version":"0.1.0",
                   "description":"Who has gone quiet, and what was last said to them.",
                   "repo":"{repo}","commit":"{commit}"}}]}}"#
        );
        if sources::list(market.config()).expect("sources").is_empty() {
            sources::add(market.config(), "adi", "https://example/marketplace.json").expect("add");
        }
        crate::sync::sync_with(market, |_| Ok(manifest.clone().into_bytes())).expect("sync");
    }

    /// A scratch store with the fixture repository published in it. Answers the market, the
    /// repository's two commits, and the scratch root.
    fn fixture(tag: &str) -> (Marketplace, String, String) {
        let market = crate::tests::scratch(tag);
        let (repo, first, second) = upstream(&market.config().root().join("publisher"), "v1\n");
        synced(&market, &repo, &first);
        (market, first, second)
    }

    #[test]
    fn an_install_clones_the_pin_under_the_name_the_operator_chose() {
        let (market, first, _second) = fixture("named");

        let done = install(&market, "adi/crm", "Sales CRM", false)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(done.id, "sales-crm", "the id is minted from the name, not the slug");
        assert_eq!(done.name, "Sales CRM");
        assert_eq!(done.host, "sales-crm.adi");
        assert_eq!(done.commit, first);
        assert!(!done.started, "an install never starts anything by itself");

        let dir = market.dashboards_dir().join("sales-crm");
        assert_eq!(
            std::fs::read_to_string(dir.join("frontend").join("modules").join("contacts.ts"))
                .expect("panel"),
            "v1\n",
            "the pinned tree, not the tip"
        );
        // It is a clone, and one that can still be pulled: the pin sits on a tracking branch.
        assert_eq!(git::head(&dir).as_deref(), Some(first.as_str()));
        assert_eq!(git::branch(&dir).as_deref(), Some("main"));
        assert!(!git::dirty(&dir), "the store's own files are excluded, not untracked work");

        // Inert on arrival, in the state the store already knows how to hold and Restore.
        assert!(!is_started(&dir), "the supervisor's glob must not match");
        assert!(dir.join(".adi").join(HIVE_ARCHIVED).is_file());
        let manifest = read_manifest(&dir);
        assert!(manifest.archived_at.is_some(), "arrival is archived");
        assert_eq!(manifest.name.as_deref(), Some("Sales CRM"));

        // And the record says where it came from and where it stands.
        let record = read_record(&dir).expect("record");
        assert_eq!((record.marketplace.as_str(), record.slug.as_str()), ("adi", "crm"));
        assert_eq!(record.commit, first);
        assert_eq!(record.branch.as_deref(), Some("main"));

        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn two_copies_of_one_app_are_ordinary() {
        let (market, _first, _second) = fixture("twice");
        let first = install(&market, "adi/crm", "CRM", false).expect("install");
        let second = install(&market, "adi/crm", "CRM", false).expect("install again");
        assert_eq!(first.id, "crm");
        assert_eq!(second.id, "crm-2", "numbered, never refused and never overwritten");
        assert_eq!(second.host, "crm-2.adi", "and they do not share a hostname");
        assert!(market.dashboards_dir().join("crm").join(".git").is_dir());
        assert!(market.dashboards_dir().join("crm-2").join(".git").is_dir());

        // An empty name falls back to the entry's own.
        let third = install(&market, "adi/crm", "   ", false).expect("unnamed");
        assert_eq!(third.name, "CRM");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn the_listing_reports_every_copy_and_which_ones_are_behind() {
        let (market, first, second) = fixture("listing");
        install(&market, "adi/crm", "CRM", false).expect("install");

        let apps = cached_apps(market.config());
        assert_eq!(apps.len(), 1);
        assert!(apps[0].repo.starts_with("file://"));
        assert_eq!(apps[0].commit, first);
        assert_eq!(apps[0].installs.len(), 1);
        assert_eq!(apps[0].installs[0].id, "crm");
        assert!(!apps[0].installs[0].started);
        assert!(!apps[0].installs[0].outdated, "installed at the pin it lists");
        assert_eq!(apps[0].installs[0].host.as_deref(), Some("crm.adi"));

        // The publisher moves the pin: the copy here is behind, and says so.
        synced(&market, &apps[0].repo.clone(), &second);
        let apps = cached_apps(market.config());
        assert!(apps[0].installs[0].outdated, "{:?}", apps[0].installs);

        // Starting flips the row.
        start(&market, "crm").expect("start");
        let apps = cached_apps(market.config());
        assert!(apps[0].installs[0].started);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_update_moves_the_copy_onto_the_new_pin() {
        let (market, first, second) = fixture("update");
        install(&market, "adi/crm", "CRM", false).expect("install");
        let dir = market.dashboards_dir().join("crm");

        // Nothing to do while the manifest still pins what is here.
        let same = update(&market, "crm", false).expect("no-op");
        assert!(!same.changed);
        assert_eq!(same.from, first);

        let repo = git::remote(&dir).expect("remote");
        synced(&market, &repo, &second);
        let done = update(&market, "crm", false).unwrap_or_else(|e| panic!("{e}"));
        assert!(done.changed);
        assert_eq!((done.from.as_str(), done.to.as_str()), (first.as_str(), second.as_str()));
        assert_eq!(
            std::fs::read_to_string(dir.join("frontend").join("modules").join("contacts.ts"))
                .expect("panel"),
            "v2\n"
        );
        let record = read_record(&dir).expect("record");
        assert_eq!(record.commit, second);
        assert!(record.updated_at.is_some());
        // The store's own files came through the update untouched.
        assert_eq!(read_manifest(&dir).name.as_deref(), Some("CRM"));
        assert!(dir.join(".adi").join(HIVE_ARCHIVED).is_file());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_update_will_not_walk_over_uncommitted_work() {
        let (market, _first, second) = fixture("dirty");
        install(&market, "adi/crm", "CRM", false).expect("install");
        let dir = market.dashboards_dir().join("crm");
        let panel = dir.join("frontend").join("modules").join("contacts.ts");
        std::fs::write(&panel, "mine\n").expect("edit");

        let repo = git::remote(&dir).expect("remote");
        synced(&market, &repo, &second);
        let err = update(&market, "crm", false).expect_err("refused");
        assert!(matches!(err, Error::Dirty(_)), "{err}");
        assert_eq!(std::fs::read_to_string(&panel).expect("panel"), "mine\n");

        // Forcing is the deliberate second ask.
        let done = update(&market, "crm", true).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(done.to, second);
        assert_eq!(std::fs::read_to_string(&panel).expect("panel"), "v2\n");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_repository_that_is_not_an_app_lands_nowhere() {
        let market = crate::tests::scratch("not-an-app");
        let root = market.config().root().join("publisher");
        let dir = root.join("upstream");
        std::fs::create_dir_all(&dir).expect("dir");
        let git_run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}");
        };
        git_run(&["init", "--quiet", "-b", "main"]);
        git_run(&["config", "user.email", "p@example"]);
        git_run(&["config", "user.name", "P"]);
        std::fs::write(dir.join("README.md"), "not a dashboard\n").expect("seed");
        git_run(&["add", "."]);
        git_run(&["commit", "--quiet", "-m", "one"]);
        synced(
            &market,
            &format!("file://{}", dir.display()),
            &head_of(&dir),
        );

        let err = install(&market, "adi/crm", "CRM", false).expect_err("refused");
        assert!(matches!(err, Error::NotAnApp(_, _)), "{err}");
        assert!(err.to_string().contains("frontend/index.ts"), "{err}");
        assert!(
            !market.dashboards_dir().join("crm").exists(),
            "a refused install left a directory behind"
        );
        assert!(
            !staging_dir(&market).join("crm").exists(),
            "and nothing in staging either"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_repository_may_not_bring_its_own_hive_file_into_the_glob() {
        let market = crate::tests::scratch("hostile-hive");
        let root = market.config().root().join("publisher");
        let (repo, _first, _second) = upstream(&root, "v1\n");
        let dir = root.join("upstream");
        std::fs::create_dir_all(dir.join(".adi")).expect("adi");
        // What a hostile app would ship: a runner of its own, in the file the supervisor reads.
        std::fs::write(
            dir.join(".adi").join("hive.yaml"),
            "version: \"1\"\nservices:\n  x:\n    runner:\n      type: script\n",
        )
        .expect("hive");
        std::fs::write(dir.join("hive.yaml"), "version: \"1\"\n").expect("root hive");
        for args in [
            &["add", "-A", "-f"][..],
            &["commit", "--quiet", "-m", "with a hive"][..],
        ] {
            let out = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        }
        synced(&market, &repo, &head_of(&dir));

        let done = install(&market, "adi/crm", "CRM", false).expect("install");
        let landed = market.dashboards_dir().join("crm");
        assert!(!landed.join("hive.yaml").exists(), "a root hive file never lands");
        assert!(
            !is_started(&landed),
            "and nothing the repository shipped is in the supervisor's glob"
        );
        let hive = std::fs::read_to_string(landed.join(".adi").join(HIVE_ARCHIVED)).expect("hive");
        assert!(hive.contains("host: crm.adi"), "{hive}");
        assert!(hive.contains("bun run frontend/index.ts"), "{hive}");
        assert!(
            done.notes.iter().any(|n| n.contains(".adi")),
            "and the operator is told: {:?}",
            done.notes
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn every_wrong_ask_is_refused_with_the_sentence_that_names_it() {
        let (market, _first, _second) = fixture("refusals");

        assert!(matches!(
            install(&market, "crm", "CRM", false),
            Err(Error::BadSpec(_))
        ));
        assert!(matches!(
            install(&market, "other/crm", "CRM", false),
            Err(Error::UnknownSource(_))
        ));
        assert!(matches!(
            install(&market, "adi/nope", "CRM", false),
            Err(Error::UnknownApp(_, slug, slugs)) if slug == "nope" && slugs.contains("crm")
        ));
        assert!(matches!(start(&market, "ghost"), Err(Error::NotInstalled(_))));
        assert!(matches!(update(&market, "ghost", false), Err(Error::NotInstalled(_))));

        // A source never synced has nothing to install from.
        let bare = crate::tests::scratch("refusals-bare");
        sources::add(bare.config(), "adi", "https://example/m.json").expect("add");
        assert!(matches!(
            install(&bare, "adi/crm", "CRM", false),
            Err(Error::NotSynced(_))
        ));
        let _ = std::fs::remove_dir_all(bare.config().root());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn start_gives_way_when_the_arrival_host_was_taken_meanwhile() {
        let (market, _first, _second) = fixture("taken");
        install(&market, "adi/crm", "CRM", false).expect("install");

        // A neighbour claims `crm.adi` after the app arrived.
        let neighbour = market.dashboards_dir().join("resident-crm");
        std::fs::create_dir_all(neighbour.join(".adi")).expect("neighbour");
        std::fs::write(
            neighbour.join(".adi").join(HIVE_LIVE),
            "version: \"1\"\nservices:\n  frontend:\n    proxy:\n      host: crm.adi\n",
        )
        .expect("hive");

        let started = start(&market, "crm").expect("start");
        // The parked file's label is only a preference: two dashboards on one hostname is a
        // routing coin-flip, so a fresh one is derived instead.
        assert_eq!(started.host, "dashboard.adi", "{}", started.host);
        let _ = std::fs::remove_dir_all(market.config().root());
    }
}
