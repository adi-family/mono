//! Installing a bundle's elements — phase B of `docs/marketplace-bundles.md`: "Installing part of
//! a bundle" through "Provenance, without one directory per element".
//!
//! [`install()`] is new code beside `crate::install`'s own pipeline, not a change to it (see that
//! document's "What v1's code assumes this design changes"): a v1 app moves one clone into one
//! directory with one `std::fs::rename`, and a general bundle's clone becomes zero, one, or many
//! store locations of at least three different shapes. The one exception is the legacy
//! single-dashboard bundle (decision #6), which this module detects and delegates to
//! [`crate::install::install`] wholesale rather than folding into the general case —
//! [`BundleOutcome::Legacy`] is how a caller tells the two apart.
//!
//! **Every kind lands through its own store's own write path** — `adi_agents::Agents::save`,
//! `adi_tools::Tools::create_file`, `adi_triggers::Triggers::save`, `adi_projects::Projects::
//! create_with_id`, `adi_agents::llm::LlmBackends::save`, `adi_embeddings::EmbeddingBackends::
//! save` — never a hand-rolled file write, so what lands can never drift from the store's own
//! invariants (`created_at`/`updated_at` stamped fresh, names validated, and so on).
//!
//! **The two collision regimes** (`docs/marketplace-bundles.md` decision #3) come down to one
//! question per kind: does landing under a fresh id ever leave a sibling's *structured* reference
//! — `bin_tools`, `backends[].backend`, `project` — silently pointing at a stranger's pre-existing
//! entry?
//!
//! * Agents, dashboards, triggers, and hive services: no. Nothing reads their id back out of a
//!   sibling's structured field, so `land_agent`/`land_dashboard`/`land_trigger`/`land_service`
//!   mint past a collision exactly like v1's dashboards.
//! * Tools, LLM backends, embedding backends, and the project scaffold: yes.
//!   `land_tool`/`land_llm`/`land_embedding`/`land_project` refuse a collision outright — the id
//!   an installed sibling's `bin_tools`/`backends`/`project` names is either exactly what the
//!   repository published, or that element simply did not land.
//!
//! Because a refuse-kind element only ever lands under its published name, **no cross-element
//! reference is ever rewritten**: an agent's `bin_tools = ["csv-import"]` is passed through
//! byte-for-byte, and it resolves correctly whenever the sibling tool landed at all. The one
//! rewrite this module does perform is `project`, which every kind's own manifest carries and
//! which a publisher cannot fill in honestly (`docs/marketplace-bundles.md` decision #4): it is
//! always struck from what the repository shipped and replaced with this bundle's own landed
//! project id, or `None` when this bundle carries no project scaffold at all.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::address::Address;
use crate::error::{Error, Result};
use crate::git;
use crate::install::{self, entry_of};
use crate::kind::Kind;
use crate::layout::{self, LayoutElement};
use crate::manifest::BundleEntry;
use crate::Marketplace;

/// The three generated dashboard entry points, excluded from a general-bundle dashboard's
/// fingerprint for the same reason v1 excludes them from its own drift check
/// (`crate::git::GENERATED`): the panel rewrites them in place, so they are never the operator's
/// (or the publisher's) edit to notice.
const GENERATED_DASHBOARD_FILES: [&str; 3] =
    ["frontend/index.html", "frontend/index.ts", "backend/index.ts"];

/// Where the permanent per-bundle state lives under the marketplace module, one directory level
/// per coordinate (`<marketplace>/<slug>`) rather than a single flattened id — the source and the
/// slug are each already one safe path segment, and joining them as a path costs nothing that
/// inventing a delimiter would not also have to pay for.
const BUNDLES_DIR: &str = "bundles";
const INSTALLS_DIR: &str = "installs";
const SERVICES_DIR: &str = "services";

/// One element as an install call answers for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ElementOutcome {
    /// Which of the eight kinds this is.
    pub kind: Kind,
    /// The published name — the file or directory stem the repository carried it under.
    pub name: String,
    /// The id it actually landed as, or `None` when it did not land at all (blocked by a
    /// collision, already installed from an earlier call, or a failure of its own).
    pub id: Option<String>,
    /// Whether the landed id differs from the published name — only ever `true` for a kind that
    /// mints freely on a collision (an agent, a dashboard, a trigger, a hive service).
    pub renamed: bool,
    /// Anything worth telling the operator about this one element: why it was blocked, that a
    /// repository-versioned stamp was dropped, that it failed outright.
    pub note: Option<String>,
}

/// What installing a general (non-legacy) bundle answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleInstalled {
    /// The source's local name — the first half of `<marketplace>/<slug>`.
    pub marketplace: String,
    /// The bundle's published slug — the second half.
    pub slug: String,
    /// This bundle's own landed project id, or `None` when it carries no project scaffold (or the
    /// scaffold has not been installed yet).
    pub project: Option<String>,
    /// Every element this call considered, in the order [`Kind::ALL`] lists the kinds.
    pub elements: Vec<ElementOutcome>,
    /// Every declared secret name (an agent's `secrets`, a backend's `api_key_env`) that this
    /// machine does not have set, global or project scope both. Not a gate — every element lands
    /// regardless (`docs/marketplace-bundles.md`, "Secrets").
    pub missing_secrets: Vec<String>,
    /// One sentence per name in [`missing_secrets`](Self::missing_secrets), phrased for the
    /// operator, the same role v1's `Installed.notes` plays for a repository-versioned stamp.
    pub notes: Vec<String>,
}

/// What [`install()`] answers with — the legacy single-dashboard bundle is decision #6's named
/// exception, kept on v1's own mechanism verbatim rather than folded into the general shape above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BundleOutcome {
    /// A legacy v1 app repository — no kind directories, a dashboard at the root. Installed
    /// exactly as `crate::install::install` has always installed one: its own clone under
    /// `dashboards/<id>/`, its own `.adi/marketplace.json`, none of this module's ledger.
    Legacy(install::Installed),
    /// A general bundle, landed element by element through this module's own pipeline.
    Bundle(BundleInstalled),
}

/// One installed element, as the ledger keeps it: enough to answer "what does this bundle have
/// installed, and under what id" without re-reading every store — and, once phase C reads it, a
/// content fingerprint to notice an operator's edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerElement {
    pub kind: Kind,
    /// The published name — how a later call recognises "this element is already installed",
    /// independent of what it was minted or refused under.
    pub name: String,
    /// The id it landed as.
    pub id: String,
    /// A content fingerprint of what was written at install — sha-256, hex, of the bytes on disk
    /// for a flat-file kind, or of every tracked file (sorted, generated entry points excluded)
    /// for a dashboard. `crate::bundle` never compares it to anything; it is written for phase C's
    /// drift detection to read.
    pub fingerprint: String,
}

/// One bundle's whole install record, at `marketplace/installs/<marketplace>/<slug>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    pub marketplace: String,
    pub slug: String,
    pub repo: String,
    pub commit: String,
    pub branch: Option<String>,
    pub installed_at: u64,
    pub updated_at: Option<u64>,
    /// Every element installed from this bundle so far — never emptied while any element remains,
    /// growing across calls exactly as "installing one more element later" describes.
    pub elements: Vec<LedgerElement>,
}

impl Ledger {
    fn fresh(marketplace: &str, slug: &str, entry: &BundleEntry, pin: &git::Pin) -> Self {
        Self {
            marketplace: marketplace.to_string(),
            slug: slug.to_string(),
            repo: entry.repo.clone(),
            commit: pin.commit.clone(),
            branch: Some(pin.branch.clone()),
            installed_at: adi_config::now_unix(),
            updated_at: None,
            elements: Vec::new(),
        }
    }

    /// The id this bundle's own project scaffold landed as, if it has been installed at all.
    #[must_use]
    pub fn project_id(&self) -> Option<String> {
        self.elements
            .iter()
            .find(|e| e.kind == Kind::Project)
            .map(|e| e.id.clone())
    }

    /// Whether an element by this published name and kind is already recorded — the question that
    /// makes a second `install` call of the same bundle grow the ledger rather than re-land what
    /// is already there.
    fn find(&self, kind: Kind, name: &str) -> Option<&LedgerElement> {
        self.elements
            .iter()
            .find(|e| e.kind == kind && e.name == name)
    }
}

/// One element's landing, before it is folded into an [`ElementOutcome`] and (on success) a
/// [`LedgerElement`].
enum Landed {
    Ok {
        id: String,
        fingerprint: String,
        renamed: bool,
        note: Option<String>,
    },
    /// A verbatim-or-refuse kind whose published name already names something else here.
    Blocked(String),
    /// Anything else that stopped this one element — a parse failure, a store's own validation
    /// refusal, an I/O error. Never the whole install's problem: the loop moves on to the next
    /// element, per "a failure partway leaves what landed landed" (`docs/marketplace-bundles.md`,
    /// "What is deliberately not here").
    Failed(String),
}

/// Install `<marketplace>/<slug>` (the whole bundle) or `<marketplace>/<slug>/<kind>/<name>` (one
/// element of it): read the pinned tree, land every element the selection names into its real
/// store location, and record what happened.
///
/// # Errors
/// [`Error::BadSpec`] / [`Error::BadAddress`] for a malformed address, [`Error::UnknownSource`] /
/// [`Error::NotSynced`] / [`Error::UnknownApp`] for a bundle that is not cached, whatever
/// [`BundleEntry::validate`] refuses, [`Error::Git`] for a clone failure, [`Error::CarriesRust`]
/// for a repository that ships Rust under a kind directory, [`Error::UnknownElement`] when a
/// single-element address names nothing the tree carries, and [`Error::EmptyBundle`] when the tree
/// carries nothing installable at all. Every other way *one element* can fail is not one of these
/// — it is reported in that element's own [`ElementOutcome::note`], and the rest of the selection
/// still lands.
pub fn install(market: &Marketplace, spec: &str) -> Result<BundleOutcome> {
    let addr = Address::parse(spec)?;
    let config = market.config();
    let entry = entry_of(config, &addr.marketplace, &addr.slug)?;
    entry.validate()?;

    let clone_dir = bundle_dir(market, &addr.marketplace, &addr.slug);
    let pin = ensure_clone(market, &addr.marketplace, &addr.slug, &clone_dir, &entry)?;
    let tree = layout::read_layout(&clone_dir, &entry.slug)?;

    if tree.legacy {
        // Decision #6: this shape keeps v1's mechanism verbatim. `crate::install::install` clones
        // its own copy under `dashboards/<id>/` — the permanent clone this function just made at
        // `marketplace/bundles/…` is not what a legacy bundle uses, and is left in place unread
        // rather than wired into anything (nothing here removes it, either: an install that
        // refused nothing left nothing to clean up).
        let done = install::install(market, spec, "", false)?;
        return Ok(BundleOutcome::Legacy(done));
    }

    let selected: Vec<&LayoutElement> = match &addr.element {
        None => tree.elements.iter().collect(),
        Some(target) => {
            let found = tree
                .elements
                .iter()
                .find(|e| e.kind == target.kind && e.name == target.name);
            match found {
                Some(el) => vec![el],
                None => return Err(Error::UnknownElement(spec.to_string())),
            }
        }
    };
    if selected.is_empty() {
        return Err(Error::EmptyBundle(entry.slug.clone()));
    }

    let mut ledger = read_ledger(market, &addr.marketplace, &addr.slug)
        .unwrap_or_else(|| Ledger::fresh(&addr.marketplace, &addr.slug, &entry, &pin));

    // The project scaffold first, whatever order the layout listed it in: every sibling's
    // `project` field is rewritten below, and it can only be rewritten to something once this
    // call knows what this bundle's project id is.
    let mut ordered = selected;
    ordered.sort_by_key(|el| el.kind != Kind::Project);

    let stores = Stores::open(market, config, &addr.marketplace, &addr.slug);
    let mut project_id = ledger.project_id();
    let mut outcomes = Vec::new();
    let mut missing_secrets: BTreeSet<String> = BTreeSet::new();

    for el in ordered {
        // The project scaffold is the one singleton — it carries no published name of its own, so
        // the bundle's own slug stands for it, the same way the address spells it with none.
        let name = el.name.clone().unwrap_or_else(|| addr.slug.clone());

        if let Some(already) = ledger.find(el.kind, &name) {
            outcomes.push(ElementOutcome {
                kind: el.kind,
                name,
                id: Some(already.id.clone()),
                renamed: false,
                note: Some("already installed by an earlier call — left as it stands".to_string()),
            });
            continue;
        }

        let landed = stores.land(el, &clone_dir, project_id.as_deref(), &addr.slug, &mut missing_secrets);
        let (outcome, ledgered) = fold(el.kind, name, landed);
        if el.kind == Kind::Project {
            project_id = ledgered.as_ref().map(|e| e.id.clone()).or(project_id);
        }
        if let Some(ledgered) = ledgered {
            ledger.elements.push(ledgered);
        }
        outcomes.push(outcome);
    }

    write_ledger(market, &addr.marketplace, &addr.slug, &ledger)?;

    let notes = missing_secrets
        .iter()
        .map(|name| {
            format!(
                "names {name}, which is not set — the bundle's elements that declare it will not \
                 work until you set it"
            )
        })
        .collect();

    Ok(BundleOutcome::Bundle(BundleInstalled {
        marketplace: addr.marketplace,
        slug: addr.slug,
        project: project_id,
        elements: outcomes,
        missing_secrets: missing_secrets.into_iter().collect(),
        notes,
    }))
}

/// One handle per store a bundle might land an element into, opened once per [`install`] call
/// rather than once per element — cheap (every one of them is a thin wrapper over [`Config`]), and
/// it is what keeps [`install`] itself down to the address/layout/ledger bookkeeping.
struct Stores {
    agents: adi_agents::Agents,
    tools: adi_tools::Tools,
    triggers: adi_triggers::Triggers,
    projects: adi_projects::Projects,
    llm_backends: adi_agents::llm::LlmBackends,
    embedding_backends: adi_embeddings::EmbeddingBackends,
    secrets: adi_secrets::Secrets,
    dashboards_dir: PathBuf,
    parked_services_dir: PathBuf,
}

impl Stores {
    fn open(market: &Marketplace, config: &adi_config::Config, marketplace: &str, slug: &str) -> Self {
        Self {
            agents: adi_agents::Agents::with_config(config.clone()),
            tools: adi_tools::Tools::with_config(config.clone()),
            triggers: adi_triggers::Triggers::with_config(config.clone()),
            projects: adi_projects::Projects::with_config(config.clone()),
            llm_backends: adi_agents::llm::LlmBackends::with_config(config.clone()),
            embedding_backends: adi_embeddings::EmbeddingBackends::with_config(config.clone()),
            secrets: adi_secrets::Secrets::with_config(config.clone()),
            dashboards_dir: market.dashboards_dir(),
            parked_services_dir: services_dir(market, marketplace, slug),
        }
    }

    /// Dispatch one element to its kind's own landing function.
    fn land(
        &self,
        el: &LayoutElement,
        root: &Path,
        project: Option<&str>,
        bundle_slug: &str,
        missing_secrets: &mut BTreeSet<String>,
    ) -> Landed {
        match el.kind {
            Kind::Agent => land_agent(&self.agents, root, el, project, &self.secrets, missing_secrets),
            Kind::Tool => land_tool(&self.tools, root, el, project),
            Kind::Dashboard => land_dashboard(&self.dashboards_dir, root, el, project),
            Kind::Llm => land_llm(&self.llm_backends, root, el, &self.secrets, missing_secrets),
            Kind::Embedding => {
                land_embedding(&self.embedding_backends, root, el, &self.secrets, missing_secrets)
            }
            Kind::Service => land_service(&self.parked_services_dir, root, el),
            Kind::Trigger => land_trigger(&self.triggers, root, el, project),
            Kind::Project => land_project(&self.projects, root, el, bundle_slug),
        }
    }
}

/// Fold one element's [`Landed`] outcome into what the caller reports and — on success — what the
/// ledger keeps.
fn fold(kind: Kind, name: String, landed: Landed) -> (ElementOutcome, Option<LedgerElement>) {
    match landed {
        Landed::Ok {
            id,
            fingerprint,
            renamed,
            note,
        } => (
            ElementOutcome {
                kind,
                name: name.clone(),
                id: Some(id.clone()),
                renamed,
                note,
            },
            Some(LedgerElement {
                kind,
                name,
                id,
                fingerprint,
            }),
        ),
        Landed::Blocked(reason) => (
            ElementOutcome {
                kind,
                name,
                id: None,
                renamed: false,
                note: Some(reason),
            },
            None,
        ),
        Landed::Failed(reason) => (
            ElementOutcome {
                kind,
                name,
                id: None,
                renamed: false,
                note: Some(format!("did not install: {reason}")),
            },
            None,
        ),
    }
}

/// The permanent clone: `marketplace/bundles/<marketplace>/<slug>/` — kept rather than discarded
/// (decision #6 is the one shape that gets its own clone elsewhere, under `dashboards/<id>/`).
fn bundle_dir(market: &Marketplace, marketplace: &str, slug: &str) -> PathBuf {
    market
        .config()
        .module(crate::MODULE)
        .dir()
        .join(BUNDLES_DIR)
        .join(marketplace)
        .join(slug)
}

/// Where a hive service element parks until something starts it:
/// `marketplace/services/<marketplace>/<slug>/<name>.yaml`, a file no supervisor glob matches.
fn services_dir(market: &Marketplace, marketplace: &str, slug: &str) -> PathBuf {
    market
        .config()
        .module(crate::MODULE)
        .dir()
        .join(SERVICES_DIR)
        .join(marketplace)
        .join(slug)
}

fn ledger_raw_name(marketplace: &str, slug: &str) -> String {
    format!("{INSTALLS_DIR}/{marketplace}/{slug}.json")
}

/// Read a bundle's ledger, or `None` for one nothing has installed from yet.
#[must_use]
pub fn read_ledger(market: &Marketplace, marketplace: &str, slug: &str) -> Option<Ledger> {
    let bytes = market
        .config()
        .module(crate::MODULE)
        .read_raw(&ledger_raw_name(marketplace, slug))
        .ok()??;
    serde_json::from_slice(&bytes).ok()
}

fn write_ledger(market: &Marketplace, marketplace: &str, slug: &str, ledger: &Ledger) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(ledger)
        .map_err(|e| Error::Fetch(format!("encoding the install ledger: {e}")))?;
    market
        .config()
        .module(crate::MODULE)
        .write_raw(&ledger_raw_name(marketplace, slug), &bytes)?;
    Ok(())
}

/// Clone the bundle's repository at its pinned commit into the permanent location, or — the
/// common case for every call after the first — answer the pin the clone already stands at.
/// Nothing here moves an existing clone onto a new pin: that is `update`'s job, not this one's.
///
/// The no-Rust scan runs on the *staged* clone, before it ever reaches the permanent location —
/// the same reason v1's own `stage` never moves a rejected clone into the dashboards tree: a
/// bundle refused whole must leave nothing behind for a later call to find half-installed.
fn ensure_clone(
    market: &Marketplace,
    marketplace: &str,
    slug: &str,
    dest: &Path,
    entry: &BundleEntry,
) -> Result<git::Pin> {
    if dest.join(".git").is_dir() {
        let commit = git::head(dest)
            .ok_or_else(|| Error::Git(format!("{} is not a working clone", dest.display())))?;
        return Ok(git::Pin {
            commit,
            branch: git::branch(dest).unwrap_or_default(),
        });
    }
    let staging = market
        .config()
        .module(crate::MODULE)
        .dir()
        .join("staging")
        .join(format!("bundle-{marketplace}-{slug}"));
    let _ = std::fs::remove_dir_all(&staging);
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let outcome = (|| -> Result<git::Pin> {
        let pin = git::clone_pinned(&entry.repo, &entry.pin(), entry.branch(), &staging)
            .map_err(Error::Git)?;
        layout::scan_for_rust(&staging, &entry.slug)?;
        Ok(pin)
    })();
    let pin = match outcome {
        Ok(pin) => pin,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Err(e) = std::fs::rename(&staging, dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Error::Io(e));
    }
    Ok(pin)
}

// MARK: secrets — declared, never a value, diffed against what this machine actually has

/// Whether `name` is set for `project` (global when `None`) — through
/// [`adi_secrets::Secrets::resolve`], the same merge (project overrides global) a run gets its
/// environment from, so "missing" here means the same thing "missing" means at launch.
fn secret_set(secrets: &adi_secrets::Secrets, project: Option<&str>, name: &str) -> bool {
    secrets
        .resolve(project)
        .map(|resolved| resolved.contains_key(name))
        .unwrap_or(false)
}

// MARK: fingerprints — sha-256, hex, of what was actually written

fn fingerprint_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fingerprint_file(path: &Path) -> String {
    fingerprint_bytes(&std::fs::read(path).unwrap_or_default())
}

/// The fingerprint of several files together — a tool's manifest and its script, say — sorted by
/// path first so the order two reads produce is never the filesystem's to decide.
fn fingerprint_files(paths: &[PathBuf]) -> String {
    let mut sorted = paths.to_vec();
    sorted.sort();
    let mut hasher = Sha256::new();
    for path in sorted {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(std::fs::read(&path).unwrap_or_default());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

/// The fingerprint of a dashboard's tracked files — every entry `adi_dashboards::collect_files`
/// walked, already sorted, the three generated entry points excluded so the panel's own rewrite of
/// them is never mistaken for an edit worth flagging (mirrors `crate::git::GENERATED`, which
/// excludes the same three from v1's own drift check).
fn fingerprint_dashboard_files(files: &[adi_dashboards::BundleFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        if GENERATED_DASHBOARD_FILES.contains(&file.path.as_str()) {
            continue;
        }
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.contents.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

// MARK: the two collision regimes

/// Why a refuse-kind collision is refused rather than minted past — folded into every
/// [`Landed::Blocked`] message so the operator reads the reasoning, not just the refusal.
fn refuse_reason(kind: Kind, name: &str) -> String {
    let why = match kind {
        Kind::Tool => {
            "a sibling agent's bin_tools may name this id verbatim, and a numbered suffix would \
             silently point it at whatever already has this name"
        }
        Kind::Llm | Kind::Embedding => {
            "an agent's backends (or an embedding assignment) may name this id verbatim, and a \
             numbered suffix would attach this bundle's configuration to the wrong login"
        }
        Kind::Project => "every other kind's project field names a project by exactly this id",
        _ => "landing under a different id would break a structured reference to it",
    };
    format!(
        "a {kind} named {name:?} already exists on this machine — refused rather than installed \
         under a different id, because {why}"
    )
}

fn strip_stamps_note(where_: &str, had_stamps: bool) -> Option<String> {
    had_stamps.then(|| format!("{where_} versions created_at/updated_at — dropped, the store stamps them fresh"))
}

// MARK: per-kind landing

fn land_agent(
    agents: &adi_agents::Agents,
    root: &Path,
    el: &LayoutElement,
    project: Option<&str>,
    secrets: &adi_secrets::Secrets,
    missing: &mut BTreeSet<String>,
) -> Landed {
    let name = el.name.clone().unwrap_or_default();
    let text = match std::fs::read_to_string(root.join(&el.path)) {
        Ok(t) => t,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    let mut manifest: adi_agents::StoredAgentManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Landed::Failed(format!("agents/{name}.toml: {e}")),
    };
    let note = strip_stamps_note(
        &format!("agents/{name}.toml"),
        manifest.created_at != 0 || manifest.updated_at != 0,
    );
    manifest.created_at = 0;
    manifest.updated_at = 0;
    manifest.project = project.map(str::to_string);
    for attachment in &manifest.secrets {
        if !secret_set(secrets, attachment.project.as_deref(), &attachment.name) {
            missing.insert(attachment.name.clone());
        }
    }

    let id = adi_config::mint(&name, "agent", |candidate| {
        agents.get(candidate).map(|found| found.is_some()).unwrap_or(true)
    });
    match agents.save(&id, manifest) {
        Ok(_) => Landed::Ok {
            fingerprint: fingerprint_file(&agents.dir().join(format!("{id}.toml"))),
            renamed: id != name,
            id,
            note,
        },
        Err(e) => Landed::Failed(e.to_string()),
    }
}

fn land_trigger(
    triggers: &adi_triggers::Triggers,
    root: &Path,
    el: &LayoutElement,
    project: Option<&str>,
) -> Landed {
    let name = el.name.clone().unwrap_or_default();
    let text = match std::fs::read_to_string(root.join(&el.path)) {
        Ok(t) => t,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    let mut manifest: adi_triggers::TriggerManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Landed::Failed(format!("triggers/{name}.toml: {e}")),
    };
    let mut note = strip_stamps_note(
        &format!("triggers/{name}.toml"),
        manifest.created_at != 0 || manifest.updated_at != 0,
    );
    manifest.created_at = 0;
    manifest.updated_at = 0;
    manifest.project = project.map(str::to_string);
    // The sharp case the operator's brief calls out by name: forced regardless of what the
    // repository ships, because a background or event trigger launches on its own the moment it
    // is enabled (`docs/marketplace-bundles.md`, "Inert on arrival, per kind").
    if manifest.enabled {
        let forced = "triggers/{name}.toml asked to arrive enabled — forced to disabled; \
                      enabling it is the deliberate act that lets it run"
            .replace("{name}", &name);
        note = Some(match note {
            Some(existing) => format!("{existing}; {forced}"),
            None => forced,
        });
    }
    manifest.enabled = false;

    let id = adi_config::mint(&name, "trigger", |candidate| {
        triggers.get(candidate).map(|found| found.is_some()).unwrap_or(true)
    });
    match triggers.save(&id, manifest) {
        Ok(_) => Landed::Ok {
            fingerprint: fingerprint_file(&triggers.dir().join(format!("{id}.toml"))),
            renamed: id != name,
            id,
            note,
        },
        Err(e) => Landed::Failed(e.to_string()),
    }
}

fn land_dashboard(
    dashboards_dir: &Path,
    root: &Path,
    el: &LayoutElement,
    project: Option<&str>,
) -> Landed {
    let name = el.name.clone().unwrap_or_default();
    let src = root.join(&el.path);
    let mut files = Vec::new();
    let mut rel = PathBuf::new();
    let mut total = 0_u64;
    if let Err(e) = adi_dashboards::collect_files(&src, &mut rel, &mut files, &mut total) {
        return Landed::Failed(e.to_string());
    }

    let id = adi_config::mint(&name, "dashboard", |candidate| {
        dashboards_dir.join(candidate).exists()
    });
    let dest = dashboards_dir.join(&id);
    let decoded = match adi_dashboards::decode_bundle(&dest, &files) {
        Ok(d) => d,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    if let Err(e) = adi_dashboards::write_import(&dest, &decoded) {
        return Landed::Failed(e.to_string());
    }
    if let Err(e) = adi_dashboards::write_manifest(
        &dest,
        &adi_dashboards::Manifest {
            name: Some(name.clone()),
            description: None,
            project: project.map(str::to_string),
            // Arrives exactly as v1's own app does: archived until started.
            archived_at: Some(adi_config::now_unix()),
            moved_to: None,
        },
    ) {
        return Landed::Failed(e.to_string());
    }
    let host = adi_dashboards::preferred_host(&dest, &name, None);
    if let Err(e) = std::fs::create_dir_all(dest.join(".adi")) {
        return Landed::Failed(e.to_string());
    }
    if let Err(e) = std::fs::write(
        dest.join(".adi").join(adi_dashboards::HIVE_ARCHIVED),
        adi_dashboards::hive_yaml(&dest, &host),
    ) {
        return Landed::Failed(e.to_string());
    }

    Landed::Ok {
        fingerprint: fingerprint_dashboard_files(&files),
        renamed: id != name,
        id,
        note: None,
    }
}

fn land_service(services_dir: &Path, root: &Path, el: &LayoutElement) -> Landed {
    let name = el.name.clone().unwrap_or_default();
    let text = match std::fs::read_to_string(root.join(&el.path)) {
        Ok(t) => t,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    // A light sanity check, not a full `Hive::load` (no import expansion, no port-command
    // preprocessing applies to a single parked fragment) — just proof the repository shipped
    // something the real store's own shape will accept once it is copied into a live hive.yaml.
    if let Err(e) = serde_yaml_ng::from_str::<adi_hive::config::ServiceSpec>(&text) {
        return Landed::Failed(format!("services/{name}.yaml is not a hive service: {e}"));
    }
    let id = adi_config::mint(&name, "service", |candidate| {
        services_dir.join(format!("{candidate}.yaml")).exists()
    });
    let dest = services_dir.join(format!("{id}.yaml"));
    if let Some(parent) = dest.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Landed::Failed(e.to_string());
    }
    if let Err(e) = std::fs::write(&dest, &text) {
        return Landed::Failed(e.to_string());
    }
    Landed::Ok {
        fingerprint: fingerprint_bytes(text.as_bytes()),
        renamed: id != name,
        id,
        note: None,
    }
}

fn land_tool(
    tools: &adi_tools::Tools,
    root: &Path,
    el: &LayoutElement,
    project: Option<&str>,
) -> Landed {
    let name = el.name.clone().unwrap_or_default();
    let path = root.join(&el.path);
    let runtime = adi_tools::runtime_from_path(&path.to_string_lossy());
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    let tool = match tools.create_file(&name, None, runtime, project.map(str::to_string), Some(content)) {
        Ok(t) => t,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    // `create_file` mints on a collision the way every store does — exactly the shortcut this
    // kind may never take. Anything but a verbatim landing is undone and reported as refused.
    if tool.id != name {
        let _ = tools.remove(&tool.id);
        return Landed::Blocked(refuse_reason(Kind::Tool, &name));
    }
    let fingerprint = fingerprint_files(&[
        tools.tool_dir(&tool.id).unwrap_or_default().join("config.toml"),
        tools.script_path(&tool.id).unwrap_or_default(),
    ]);
    Landed::Ok {
        id: tool.id,
        fingerprint,
        renamed: false,
        note: None,
    }
}

fn land_llm(
    backends: &adi_agents::llm::LlmBackends,
    root: &Path,
    el: &LayoutElement,
    secrets: &adi_secrets::Secrets,
    missing: &mut BTreeSet<String>,
) -> Landed {
    let name = el.name.clone().unwrap_or_default();
    if matches!(backends.get(&name), Ok(Some(_))) {
        return Landed::Blocked(refuse_reason(Kind::Llm, &name));
    }
    let text = match std::fs::read_to_string(root.join(&el.path)) {
        Ok(t) => t,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    let mut manifest: adi_agents::llm::LlmBackendManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Landed::Failed(format!("llm/{name}.toml: {e}")),
    };
    let note = strip_stamps_note(
        &format!("llm/{name}.toml"),
        manifest.created_at != 0 || manifest.updated_at != 0,
    );
    manifest.created_at = 0;
    manifest.updated_at = 0;
    if let Some(key) = manifest.api_key_env.clone()
        && !secret_set(secrets, None, &key)
    {
        missing.insert(key);
    }
    match backends.save(&name, manifest) {
        Ok(_) => Landed::Ok {
            fingerprint: fingerprint_file(&backends.dir().join(format!("{name}.toml"))),
            renamed: false,
            id: name,
            note,
        },
        Err(e) => Landed::Failed(e.to_string()),
    }
}

fn land_embedding(
    backends: &adi_embeddings::EmbeddingBackends,
    root: &Path,
    el: &LayoutElement,
    secrets: &adi_secrets::Secrets,
    missing: &mut BTreeSet<String>,
) -> Landed {
    let name = el.name.clone().unwrap_or_default();
    if matches!(backends.get(&name), Ok(Some(_))) {
        return Landed::Blocked(refuse_reason(Kind::Embedding, &name));
    }
    let text = match std::fs::read_to_string(root.join(&el.path)) {
        Ok(t) => t,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    let mut manifest: adi_embeddings::EmbeddingBackendManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Landed::Failed(format!("embeddings/{name}.toml: {e}")),
    };
    let note = strip_stamps_note(
        &format!("embeddings/{name}.toml"),
        manifest.created_at != 0 || manifest.updated_at != 0,
    );
    manifest.created_at = 0;
    manifest.updated_at = 0;
    if let Some(key) = manifest.api_key_env.clone()
        && !secret_set(secrets, None, &key)
    {
        missing.insert(key);
    }
    match backends.save(&name, manifest) {
        Ok(_) => Landed::Ok {
            fingerprint: fingerprint_file(&backends.dir().join(format!("{name}.toml"))),
            renamed: false,
            id: name,
            note,
        },
        Err(e) => Landed::Failed(e.to_string()),
    }
}

fn land_project(
    projects: &adi_projects::Projects,
    root: &Path,
    el: &LayoutElement,
    bundle_slug: &str,
) -> Landed {
    let text = match std::fs::read_to_string(root.join(&el.path)) {
        Ok(t) => t,
        Err(e) => return Landed::Failed(e.to_string()),
    };
    let manifest: adi_projects::Manifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Landed::Failed(format!("project/config.toml: {e}")),
    };
    let name = adi_config::clean(Some(manifest.name));
    match projects.create_with_id(bundle_slug, name, manifest.description, manifest.parent) {
        Ok(project) => Landed::Ok {
            fingerprint: fingerprint_file(&projects.project_dir(&project.id).unwrap_or_default().join("config.toml")),
            renamed: false,
            id: project.id,
            note: None,
        },
        Err(adi_projects::Error::Exists(_)) => Landed::Blocked(refuse_reason(Kind::Project, bundle_slug)),
        Err(e) => Landed::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // MARK: fixture plumbing — a bundle repository this test controls, synced like a real one

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn git_init(dir: &Path) {
        std::fs::create_dir_all(dir).expect("dir");
        git(dir, &["init", "--quiet", "-b", "main"]);
        git(dir, &["config", "user.email", "publisher@example"]);
        git(dir, &["config", "user.name", "The Publisher"]);
    }

    fn git_commit(dir: &Path, msg: &str) -> String {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "--quiet", "-m", msg]);
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write_agent(root: &Path, name: &str, extra: &str) {
        std::fs::create_dir_all(root.join("agents")).expect("dir");
        std::fs::write(
            root.join("agents").join(format!("{name}.toml")),
            format!("backend = \"harness:adi\"\n{extra}"),
        )
        .expect("agent");
    }

    fn write_tool(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("tools")).expect("dir");
        std::fs::write(root.join("tools").join(format!("{name}.sh")), "#!/bin/sh\necho hi\n")
            .expect("tool");
    }

    fn write_dashboard(root: &Path, name: &str) {
        let dir = root.join("dashboards").join(name);
        std::fs::create_dir_all(dir.join("frontend")).expect("frontend");
        std::fs::create_dir_all(dir.join("backend")).expect("backend");
        std::fs::write(dir.join("frontend").join("index.ts"), "// front\n").expect("front");
        std::fs::write(dir.join("backend").join("index.ts"), "// back\n").expect("back");
    }

    fn write_llm(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("llm")).expect("dir");
        std::fs::write(root.join("llm").join(format!("{name}.toml")), "runtime = \"harness:adi\"\n")
            .expect("llm");
    }

    fn write_embedding(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("embeddings")).expect("dir");
        std::fs::write(
            root.join("embeddings").join(format!("{name}.toml")),
            "runtime = \"hash\"\nmodel = \"hash-bow-256\"\ndimensions = 256\n",
        )
        .expect("embedding");
    }

    fn write_service(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("services")).expect("dir");
        std::fs::write(
            root.join("services").join(format!("{name}.yaml")),
            "proxy:\n  host: redis.adi\n",
        )
        .expect("service");
    }

    fn write_trigger(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("triggers")).expect("dir");
        std::fs::write(
            root.join("triggers").join(format!("{name}.toml")),
            "kind = \"background\"\ncode = \"true\"\nenabled = true\n",
        )
        .expect("trigger");
    }

    fn write_project(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("project")).expect("dir");
        std::fs::write(root.join("project").join("config.toml"), format!("name = \"{name}\"\n"))
            .expect("project");
    }

    /// Sync a source called `adi` whose one bundle is `slug`, standing at `commit` of `repo`.
    fn synced(market: &Marketplace, slug: &str, repo: &str, commit: &str) {
        let manifest = format!(
            r#"{{"name":"t","bundles":[{{"slug":"{slug}","name":"{slug}","repo":"{repo}","commit":"{commit}"}}]}}"#
        );
        if crate::sources::list(market.config()).expect("sources").is_empty() {
            crate::sources::add(market.config(), "adi", "https://example/marketplace.json")
                .expect("add");
        }
        crate::sync::sync_with(market, |_| Ok(manifest.clone().into_bytes())).expect("sync");
    }

    /// Build and sync a repository at `root/upstream`, populated by `build`, and answer
    /// `(market, repo url, commit)`.
    fn fixture(tag: &str, slug: &str, build: impl FnOnce(&Path)) -> (Marketplace, String, String) {
        let market = crate::tests::scratch(tag);
        let dir = market.config().root().join("publisher").join("upstream");
        git_init(&dir);
        build(&dir);
        let commit = git_commit(&dir, "v1");
        let repo = format!("file://{}", dir.display());
        synced(&market, slug, &repo, &commit);
        (market, repo, commit)
    }

    fn bundle_dir_of(market: &Marketplace, slug: &str) -> PathBuf {
        bundle_dir(market, "adi", slug)
    }

    fn unwrap_bundle(outcome: BundleOutcome) -> BundleInstalled {
        match outcome {
            BundleOutcome::Bundle(installed) => installed,
            BundleOutcome::Legacy(_) => panic!("expected a general bundle, got the legacy path"),
        }
    }

    fn outcome_of<'a>(installed: &'a BundleInstalled, kind: Kind, name: &str) -> &'a ElementOutcome {
        installed
            .elements
            .iter()
            .find(|e| e.kind == kind && e.name == name)
            .unwrap_or_else(|| panic!("no outcome for {kind}/{name}: {installed:?}"))
    }

    // MARK: the tests

    #[test]
    fn a_full_bundle_lands_every_kind_through_its_own_store() {
        let (market, _repo, commit) = fixture("full", "crm-suite", |root| {
            write_project(root, "CRM Suite");
            write_agent(root, "sales-bot", "bin_tools = [\"csv-import\"]\n");
            write_tool(root, "csv-import");
            write_dashboard(root, "crm");
            write_llm(root, "gpt5");
            write_embedding(root, "e5");
            write_service(root, "redis");
            write_trigger(root, "nightly");
        });

        let installed = unwrap_bundle(install(&market, "adi/crm-suite").expect("install"));
        assert_eq!(installed.marketplace, "adi");
        assert_eq!(installed.slug, "crm-suite");
        assert_eq!(installed.elements.len(), 8, "{:?}", installed.elements);
        assert!(installed.elements.iter().all(|e| e.id.is_some()), "{:?}", installed.elements);

        // The project landed under the bundle's own slug, and every sibling was filed under it.
        assert_eq!(installed.project.as_deref(), Some("crm-suite"));
        let agent_id = outcome_of(&installed, Kind::Agent, "sales-bot").id.clone().expect("agent id");
        let agent = adi_agents::Agents::with_config(market.config().clone())
            .get(&agent_id)
            .expect("get")
            .expect("present");
        assert_eq!(agent.manifest.project.as_deref(), Some("crm-suite"));
        // The tool landed verbatim, so the untouched `bin_tools` reference already resolves.
        assert_eq!(agent.manifest.bin_tools, vec!["csv-import".to_string()]);

        let tool = adi_tools::Tools::with_config(market.config().clone())
            .get("csv-import")
            .expect("get")
            .expect("present");
        assert_eq!(tool.id, "csv-import", "a refuse-kind lands under its exact published name");

        let dashboard_id = outcome_of(&installed, Kind::Dashboard, "crm").id.clone().expect("id");
        assert!(market.dashboards_dir().join(&dashboard_id).join("frontend").join("index.ts").is_file());
        assert!(
            market
                .dashboards_dir()
                .join(&dashboard_id)
                .join(".adi")
                .join("hive.yaml.archived")
                .is_file(),
            "arrives inert, exactly like v1's own app"
        );

        assert!(
            adi_agents::llm::LlmBackends::with_config(market.config().clone())
                .get("gpt5")
                .expect("get")
                .is_some()
        );
        assert!(
            adi_embeddings::EmbeddingBackends::with_config(market.config().clone())
                .get("e5")
                .expect("get")
                .is_some()
        );

        // A hive service is parked, never in the live hive.yaml.
        let parked = services_dir(&market, "adi", "crm-suite").join("redis.yaml");
        assert!(parked.is_file());
        assert!(!market.config().module("hive").dir().join("hive.yaml").exists());

        // A trigger arrives forced disabled, whatever the repository shipped.
        let trigger_id = outcome_of(&installed, Kind::Trigger, "nightly").id.clone().expect("id");
        let trigger = adi_triggers::Triggers::with_config(market.config().clone())
            .get(&trigger_id)
            .expect("get")
            .expect("present");
        assert!(!trigger.manifest.enabled, "forced inert on arrival");

        // Provenance: the permanent clone, kept — and the ledger, naming every element.
        let clone_dir = bundle_dir_of(&market, "crm-suite");
        assert!(clone_dir.join(".git").is_dir());
        assert_eq!(git::head(&clone_dir).as_deref(), Some(commit.as_str()));
        let ledger = read_ledger(&market, "adi", "crm-suite").expect("ledger");
        assert_eq!(ledger.elements.len(), 8);
        assert!(ledger.elements.iter().all(|e| !e.fingerprint.is_empty()));
        assert_eq!(ledger.commit, commit);

        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn without_a_project_scaffold_every_sibling_lands_unfiled() {
        let (market, ..) = fixture("no-project", "solo-agent", |root| {
            write_agent(root, "solo", "");
        });
        let installed = unwrap_bundle(install(&market, "adi/solo-agent").expect("install"));
        assert_eq!(installed.project, None);
        let id = outcome_of(&installed, Kind::Agent, "solo").id.clone().expect("id");
        let agent = adi_agents::Agents::with_config(market.config().clone())
            .get(&id)
            .expect("get")
            .expect("present");
        assert_eq!(agent.manifest.project, None);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_repository_versioned_stamp_is_dropped_with_a_note() {
        let (market, ..) = fixture("stamped", "stamped-bundle", |root| {
            write_agent(root, "old", "created_at = 111\nupdated_at = 222\n");
        });
        let installed = unwrap_bundle(install(&market, "adi/stamped-bundle").expect("install"));
        let outcome = outcome_of(&installed, Kind::Agent, "old");
        assert!(
            outcome.note.as_deref().is_some_and(|n| n.contains("created_at/updated_at")),
            "{outcome:?}"
        );
        let agent = adi_agents::Agents::with_config(market.config().clone())
            .get(outcome.id.as_deref().expect("id"))
            .expect("get")
            .expect("present");
        assert_ne!(agent.manifest.created_at, 111, "stamped fresh, not the repository's own");
        assert!(agent.manifest.created_at > 0);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn agents_dashboards_triggers_and_services_mint_freely_on_a_collision() {
        let (market, ..) = fixture("mint", "mint-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_dashboard(root, "crm");
            write_trigger(root, "nightly");
            write_service(root, "redis");
        });
        let agents = adi_agents::Agents::with_config(market.config().clone());
        agents
            .save("sales-bot", adi_agents::StoredAgentManifest::default())
            .expect("seed a stranger at the same id");
        std::fs::create_dir_all(market.dashboards_dir().join("crm")).expect("seed dashboard dir");
        adi_triggers::Triggers::with_config(market.config().clone())
            .save("nightly", adi_triggers::TriggerManifest::default())
            .expect("seed a stranger trigger");
        std::fs::create_dir_all(services_dir(&market, "adi", "mint-bundle")).expect("dir");
        std::fs::write(
            services_dir(&market, "adi", "mint-bundle").join("redis.yaml"),
            "proxy:\n  host: somebody-elses.adi\n",
        )
        .expect("seed a stranger service");

        let installed = unwrap_bundle(install(&market, "adi/mint-bundle").expect("install"));
        for (kind, name) in [
            (Kind::Agent, "sales-bot"),
            (Kind::Dashboard, "crm"),
            (Kind::Trigger, "nightly"),
            (Kind::Service, "redis"),
        ] {
            let outcome = outcome_of(&installed, kind, name);
            let id = outcome.id.as_deref().unwrap_or_else(|| panic!("{kind}/{name} was blocked: {outcome:?}"));
            assert_ne!(id, name, "{kind}/{name} should have been minted past the stranger");
            assert!(outcome.renamed, "{kind:?}/{name}: {outcome:?}");
        }
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_tool_collision_is_refused_not_renamed() {
        let (market, ..) = fixture("tool-collide", "tool-bundle", |root| {
            write_tool(root, "csv-import");
        });
        let tools = adi_tools::Tools::with_config(market.config().clone());
        tools
            .create_file("csv-import", None, "sh", None, Some("#!/bin/sh\necho stranger\n".into()))
            .expect("seed a stranger tool at the same id");

        let installed = unwrap_bundle(install(&market, "adi/tool-bundle").expect("install"));
        let outcome = outcome_of(&installed, Kind::Tool, "csv-import");
        assert!(outcome.id.is_none(), "{outcome:?}");
        assert!(outcome.note.as_deref().is_some_and(|n| n.contains("already exists")), "{outcome:?}");

        // The stranger's script is untouched — a refuse-kind collision never overwrites.
        assert_eq!(tools.read_script("csv-import").expect("read"), "#!/bin/sh\necho stranger\n");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_llm_backend_collision_is_refused_not_overwritten() {
        let (market, ..) = fixture("llm-collide", "llm-bundle", |root| {
            write_llm(root, "gpt5");
        });
        let backends = adi_agents::llm::LlmBackends::with_config(market.config().clone());
        backends
            .save(
                "gpt5",
                adi_agents::llm::LlmBackendManifest {
                    runtime: "pty:codex".into(),
                    ..Default::default()
                },
            )
            .expect("seed a stranger backend at the same id");

        let installed = unwrap_bundle(install(&market, "adi/llm-bundle").expect("install"));
        let outcome = outcome_of(&installed, Kind::Llm, "gpt5");
        assert!(outcome.id.is_none(), "{outcome:?}");

        let stranger = backends.get("gpt5").expect("get").expect("present");
        assert_eq!(stranger.manifest.runtime.to_string(), "pty:codex", "untouched by the refusal");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_embedding_backend_collision_is_refused_not_overwritten() {
        let (market, ..) = fixture("embed-collide", "embed-bundle", |root| {
            write_embedding(root, "e5");
        });
        let backends = adi_embeddings::EmbeddingBackends::with_config(market.config().clone());
        backends
            .save(
                "e5",
                adi_embeddings::EmbeddingBackendManifest {
                    runtime: adi_embeddings::backend::Runtime::Hash,
                    model: "hash-bow-256".into(),
                    dimensions: 256,
                    label: "the stranger's".into(),
                    ..Default::default()
                },
            )
            .expect("seed a stranger backend at the same id");

        let installed = unwrap_bundle(install(&market, "adi/embed-bundle").expect("install"));
        let outcome = outcome_of(&installed, Kind::Embedding, "e5");
        assert!(outcome.id.is_none(), "{outcome:?}");
        let stranger = backends.get("e5").expect("get").expect("present");
        assert_eq!(stranger.manifest.label, "the stranger's");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_project_scaffold_collision_is_refused() {
        let (market, ..) = fixture("project-collide", "proj-bundle", |root| {
            write_project(root, "Proj Bundle");
        });
        adi_projects::Projects::with_config(market.config().clone())
            .create_with_id("proj-bundle", Some("A stranger".into()), None, None)
            .expect("seed a stranger project at the same id");

        let installed = unwrap_bundle(install(&market, "adi/proj-bundle").expect("install"));
        let outcome = outcome_of(&installed, Kind::Project, "proj-bundle");
        assert!(outcome.id.is_none(), "{outcome:?}");
        assert_eq!(installed.project, None, "nothing landed to file a sibling under");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_declared_secret_that_is_not_set_lands_the_element_anyway_and_is_reported() {
        let (market, ..) = fixture("secret", "secret-bundle", |root| {
            write_agent(root, "sales-bot", "[[secrets]]\nname = \"OPENAI_API_KEY\"\n");
        });
        let installed = unwrap_bundle(install(&market, "adi/secret-bundle").expect("install"));
        let outcome = outcome_of(&installed, Kind::Agent, "sales-bot");
        assert!(outcome.id.is_some(), "lands regardless — a secret is a report, not a gate");
        assert_eq!(installed.missing_secrets, vec!["OPENAI_API_KEY".to_string()]);
        assert!(installed.notes[0].contains("OPENAI_API_KEY"));
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_agent_naming_a_tool_the_operator_did_not_install_is_not_an_error() {
        let (market, ..) = fixture("dangling", "dangling-bundle", |root| {
            write_agent(root, "sales-bot", "bin_tools = [\"csv-import\"]\n");
            write_tool(root, "csv-import");
        });
        // Only the agent — the tool that would resolve `bin_tools` is left out of this install.
        let installed = unwrap_bundle(install(&market, "adi/dangling-bundle/agents/sales-bot").expect("install"));
        assert_eq!(installed.elements.len(), 1);
        let outcome = &installed.elements[0];
        assert!(outcome.id.is_some(), "the agent lands regardless");
        let agent = adi_agents::Agents::with_config(market.config().clone())
            .get(outcome.id.as_deref().unwrap())
            .expect("get")
            .expect("present");
        assert_eq!(agent.manifest.bin_tools, vec!["csv-import".to_string()]);
        assert!(
            adi_tools::Tools::with_config(market.config().clone())
                .get("csv-import")
                .expect("get")
                .is_none(),
            "the tool was never installed"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn installing_the_same_bundle_twice_grows_the_ledger_rather_than_relanding() {
        let (market, ..) = fixture("grow", "grow-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        let first = unwrap_bundle(install(&market, "adi/grow-bundle").expect("first"));
        let first_id = outcome_of(&first, Kind::Agent, "sales-bot").id.clone().expect("id");

        let second = unwrap_bundle(install(&market, "adi/grow-bundle").expect("second"));
        let outcome = outcome_of(&second, Kind::Agent, "sales-bot");
        assert_eq!(outcome.id.as_deref(), Some(first_id.as_str()));
        assert!(outcome.note.as_deref().is_some_and(|n| n.contains("already installed")));

        let ledger = read_ledger(&market, "adi", "grow-bundle").expect("ledger");
        assert_eq!(ledger.elements.len(), 1, "not duplicated by the second call");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_single_element_address_installs_only_that_element() {
        let (market, ..) = fixture("single", "single-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_tool(root, "csv-import");
        });
        let installed = unwrap_bundle(install(&market, "adi/single-bundle/tools/csv-import").expect("install"));
        assert_eq!(installed.elements.len(), 1);
        assert_eq!(installed.elements[0].kind, Kind::Tool);
        assert!(
            adi_agents::Agents::with_config(market.config().clone())
                .get("sales-bot")
                .expect("get")
                .is_none(),
            "the agent was never asked for"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_address_naming_no_real_element_is_refused_by_name() {
        let (market, ..) = fixture("unknown-el", "unknown-el-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        let err = install(&market, "adi/unknown-el-bundle/agents/nope").expect_err("refused");
        assert!(matches!(err, Error::UnknownElement(_)), "{err}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_bundle_carrying_rust_under_a_kind_directory_is_refused_whole() {
        let (market, ..) = fixture("rust", "rust-bundle", |root| {
            write_agent(root, "sales-bot", "");
            std::fs::create_dir_all(root.join("tools")).expect("dir");
            std::fs::write(root.join("tools").join("evil.rs"), "fn main() {}\n").expect("rs");
        });
        let err = install(&market, "adi/rust-bundle").expect_err("refused");
        assert!(matches!(err, Error::CarriesRust(_, _, _)), "{err}");
        assert!(
            !bundle_dir_of(&market, "rust-bundle").exists(),
            "nothing was kept from a refused install"
        );
        assert!(
            adi_agents::Agents::with_config(market.config().clone())
                .get("sales-bot")
                .expect("get")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_legacy_v1_repository_installs_through_v1s_own_path_untouched() {
        let (market, ..) = fixture("legacy", "crm", |root| {
            // No kind directories at all: the v1 shape, a dashboard at the repository root.
            std::fs::create_dir_all(root.join("frontend")).expect("frontend");
            std::fs::create_dir_all(root.join("backend")).expect("backend");
            std::fs::write(root.join("frontend").join("index.ts"), "// front\n").expect("f");
            std::fs::write(root.join("backend").join("index.ts"), "// back\n").expect("b");
        });

        let outcome = install(&market, "adi/crm").expect("install");
        let done = match outcome {
            BundleOutcome::Legacy(done) => done,
            BundleOutcome::Bundle(_) => panic!("a legacy repository must take v1's own path"),
        };
        assert_eq!(done.id, "crm");
        assert!(market.dashboards_dir().join("crm").join(".git").is_dir(), "v1's own clone");
        assert!(
            crate::install::read_record(&market.dashboards_dir().join("crm")).is_some(),
            "v1's own .adi/marketplace.json"
        );
        assert!(
            read_ledger(&market, "adi", "crm").is_none(),
            "none of this module's ledger machinery applies to the legacy shape"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }
}
