//! Installing, updating, uninstalling and starting a bundle's elements — phase B of
//! `docs/marketplace-bundles.md` ("Installing part of a bundle" through "Provenance, without one
//! directory per element") and phase C ("Update" through "Uninstall").
//!
//! [`update`], [`uninstall_element`] and [`start_service`] are [`install`]'s own counterparts, one
//! ledger entry at a time: an update re-lands every already-installed element from the clone's
//! fresh pin (per element, not all-or-nothing — a drifted element is left alone and reported,
//! [`Relanded`] is [`Landed`]'s mirror for it), an uninstall removes one element by its own kind's
//! ordinary path and drops it from the ledger, and starting a service copies its parked
//! `ServiceSpec` block into the target `hive.yaml`. None of the three touch the legacy
//! single-dashboard bundle (decision #6): it keeps no ledger, so [`read_ledger`] answers `None` for
//! it and [`update`]/[`uninstall_element`] refuse with [`Error::BundleNotInstalled`] — it keeps
//! updating through `crate::install::update`, by its own dashboard id, unchanged.
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

use crate::address::{Address, ElementAddress};
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

    /// The position of the ledger element an [`ElementAddress`] names — every kind matches by its
    /// published name, except the [`Kind::Project`] singleton, which the address carries no name
    /// for at all and the ledger keeps filed under the bundle's own slug instead. A position
    /// rather than the element itself, so a caller that is about to remove it never needs a second
    /// pass to relocate what it just found.
    fn position_addressed(&self, target: &ElementAddress) -> Option<usize> {
        self.elements.iter().position(|e| {
            e.kind == target.kind
                && (target.kind == Kind::Project || target.name.as_deref() == Some(e.name.as_str()))
        })
    }
}

/// One element's own outcome from an [`update`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ElementUpdateOutcome {
    pub kind: Kind,
    /// The published name — how [`update`] matches this ledger entry against the fresh pin's tree.
    pub name: String,
    /// The id it is landed as, unchanged by an update (only a fresh install ever mints one).
    pub id: String,
    /// Whether this element's content actually moved.
    pub changed: bool,
    /// Anything worth telling the operator: that it was left alone because it was edited, that an
    /// edit was overwritten because `--force` named it, that the new pin no longer publishes it.
    pub note: Option<String>,
}

/// What updating a general (non-legacy) bundle answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleUpdated {
    pub marketplace: String,
    pub slug: String,
    /// The commit this bundle's internal clone stood at before the update.
    pub from: String,
    /// The commit it stands at now.
    pub to: String,
    /// Whether the internal clone actually moved — an update onto the pin it already had is not a
    /// failure, and touches no element.
    pub changed: bool,
    /// Every installed element this call considered, in ledger order.
    pub elements: Vec<ElementUpdateOutcome>,
}

/// What [`uninstall_element`] answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ElementUninstalled {
    pub marketplace: String,
    pub slug: String,
    pub kind: Kind,
    pub name: String,
    pub id: String,
    /// Set when this was the last element the ledger carried: the ledger entry and the permanent
    /// clone were removed along with it.
    pub bundle_removed: bool,
    /// Anything worth telling the operator — a `runner.docker` service whose container keeps
    /// running, say.
    pub note: Option<String>,
}

/// What [`start_service`] answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceStarted {
    pub marketplace: String,
    pub slug: String,
    pub name: String,
    pub id: String,
    /// The project whose `.adi/hive.yaml` the service's block now lives in, or `None` for the
    /// global `hive.yaml` — the bundle's own project, when it carries one (`docs/marketplace-bundles.md`
    /// decision #4).
    pub project: Option<String>,
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

/// One already-installed element's relanding, before it is folded into an
/// [`ElementUpdateOutcome`] and — on success — the fresh fingerprint the ledger keeps.
enum Relanded {
    Ok {
        fingerprint: String,
        /// Whether the content on disk actually moved — fast-forwarding onto a pin that never
        /// touched this element is a success too, just not a change.
        changed: bool,
        note: Option<String>,
    },
    /// Left alone rather than overwritten: a drifted fingerprint without `--force`, a kind with no
    /// in-place update path, or an element the new pin no longer publishes.
    Skipped(String),
    /// Anything else that stopped this one element's update — a parse failure, a store's own
    /// validation refusal, an I/O error. Never the whole update's problem, per the same rule
    /// [`Landed::Failed`] follows for a fresh install.
    Failed(String),
}

/// Install `<marketplace>/<slug>` (the whole bundle) or `<marketplace>/<slug>/<kind>/<name>` (one
/// element of it): read the pinned tree, land every element the selection names into its real
/// store location, and record what happened.
///
/// `name` and `start_it` are the legacy single-dashboard bundle's own knobs — what to call the
/// copy, and whether to start it in the same act — passed straight through to
/// [`crate::install::install`] when the tree turns out to be that shape. Neither means anything
/// for a general bundle: an element lands under its published name, never the operator's (decision
/// #3's own reasoning), and "start" is a per-kind question this function's own [`BundleInstalled`]
/// has no single answer for.
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
pub fn install(market: &Marketplace, spec: &str, name: &str, start_it: bool) -> Result<BundleOutcome> {
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
        let done = install::install(market, spec, name, start_it)?;
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

/// Update `<marketplace>/<slug>`: fast-forward the bundle's own internal clone onto its manifest's
/// current pin, then re-apply every ledgered element from that clone into its live location — per
/// element, not all-or-nothing (`docs/marketplace-bundles.md`, "Update").
///
/// A flat-file or script element whose live fingerprint still matches what was last written is
/// fast-forwarded; one that has drifted (the operator edited it) is left alone and reported, unless
/// `force` names it — `(kind, published name)` pairs, exactly the coordinates [`LedgerElement`]
/// keeps. A dashboard gets the same treatment over its tracked files, the three generated entry
/// points excluded. A project scaffold is never relanded in place: nothing in `adi_projects`
/// exposes an in-place rewrite of a project's name or description outside creation, so it is always
/// reported rather than silently skipped.
///
/// Nothing here applies to the legacy single-dashboard bundle (decision #6): it keeps no ledger, so
/// [`read_ledger`] answers `None` for it and this refuses with [`Error::BundleNotInstalled`] —
/// exactly as it would for a slug nothing has ever installed. It keeps updating through
/// [`crate::install::update`], by its own dashboard id, unchanged.
///
/// # Errors
/// [`Error::BundleNotInstalled`] when nothing is installed from this address yet, whatever
/// [`entry_of`] and [`BundleEntry::validate`] refuse, and [`Error::Git`] when the internal clone's
/// own fast-forward fails. Anything one element's own relanding can fail on is reported in that
/// element's own [`ElementUpdateOutcome::note`] instead — the rest of the ledger still updates.
pub fn update(
    market: &Marketplace,
    marketplace: &str,
    slug: &str,
    force: &[(Kind, &str)],
) -> Result<BundleUpdated> {
    let config = market.config();
    let mut ledger = read_ledger(market, marketplace, slug)
        .ok_or_else(|| Error::BundleNotInstalled(marketplace.to_string(), slug.to_string()))?;
    let entry = entry_of(config, marketplace, slug)?;
    entry.validate()?;

    let clone_dir = bundle_dir(market, marketplace, slug);
    let from = ledger.commit.clone();
    let to = entry.pin();
    let branch = entry.branch().or(ledger.branch.as_deref()).map(str::to_string);
    let pin = git::move_to(&clone_dir, &to, branch.as_deref(), false).map_err(Error::Git)?;

    if pin.commit.eq_ignore_ascii_case(&from) {
        // Nothing moved, so nothing was re-read from the tree: every element answers exactly as
        // installed, the same "already at the pin" no-op v1's own update reports.
        let elements = ledger
            .elements
            .iter()
            .map(|e| ElementUpdateOutcome {
                kind: e.kind,
                name: e.name.clone(),
                id: e.id.clone(),
                changed: false,
                note: None,
            })
            .collect();
        return Ok(BundleUpdated {
            marketplace: marketplace.to_string(),
            slug: slug.to_string(),
            from,
            to: pin.commit,
            changed: false,
            elements,
        });
    }

    let tree = layout::read_layout(&clone_dir, slug)?;
    let stores = Stores::open(market, config, marketplace, slug);
    let project = ledger.project_id();
    let target_hive = target_hive_path(market, &stores.projects, project.as_deref())?;

    let mut outcomes = Vec::with_capacity(ledger.elements.len());
    for i in 0..ledger.elements.len() {
        let el = ledger.elements[i].clone();
        let forced = force.iter().any(|(k, n)| *k == el.kind && *n == el.name.as_str());
        let layout_el = tree.elements.iter().find(|le| {
            le.kind == el.kind && (el.kind == Kind::Project || le.name.as_deref() == Some(el.name.as_str()))
        });
        let (outcome, new_fingerprint) = match layout_el {
            None => (
                ElementUpdateOutcome {
                    kind: el.kind,
                    name: el.name.clone(),
                    id: el.id.clone(),
                    changed: false,
                    note: Some("no longer published at the new pin — left as it stands".to_string()),
                },
                None,
            ),
            Some(layout_el) => {
                let relanded = stores.reland(&el, layout_el, &clone_dir, project.as_deref(), &target_hive, forced);
                fold_update(el.kind, el.name.clone(), el.id.clone(), relanded)
            }
        };
        if let Some(fingerprint) = new_fingerprint {
            ledger.elements[i].fingerprint = fingerprint;
        }
        outcomes.push(outcome);
    }

    ledger.commit.clone_from(&pin.commit);
    ledger.branch = Some(pin.branch);
    ledger.updated_at = Some(adi_config::now_unix());
    write_ledger(market, marketplace, slug, &ledger)?;

    Ok(BundleUpdated {
        marketplace: marketplace.to_string(),
        slug: slug.to_string(),
        from,
        to: pin.commit,
        changed: true,
        elements: outcomes,
    })
}

/// Uninstall one element of `<marketplace>/<slug>/<kind>/<name>` by its own kind's ordinary path
/// (`docs/marketplace-bundles.md`, "Uninstall"), drop it from the ledger, and leave every sibling
/// untouched. The ledger entry and the permanent clone go only once nothing from the bundle remains
/// installed anywhere.
///
/// # Errors
/// [`Error::BadAddress`] when `spec` names the whole bundle rather than one element (there is no
/// marketplace-wide uninstall verb), [`Error::BundleNotInstalled`] when nothing is installed from
/// this bundle, [`Error::ElementNotInstalled`] when the address names nothing in the ledger,
/// [`Error::EmbeddingInUse`] for an embedding backend a consumer still resolves through, and
/// [`Error::Store`] for anything else the owning store refused.
pub fn uninstall_element(market: &Marketplace, spec: &str) -> Result<ElementUninstalled> {
    let addr = Address::parse(spec)?;
    let Some(target) = &addr.element else {
        return Err(Error::BadAddress(spec.to_string()));
    };
    let mut ledger = read_ledger(market, &addr.marketplace, &addr.slug)
        .ok_or_else(|| Error::BundleNotInstalled(addr.marketplace.clone(), addr.slug.clone()))?;
    let idx = ledger
        .position_addressed(target)
        .ok_or_else(|| Error::ElementNotInstalled(spec.to_string()))?;
    let element = ledger.elements[idx].clone();
    let config = market.config();

    let note = match element.kind {
        Kind::Agent => {
            adi_agents::Agents::with_config(config.clone())
                .delete(&element.id)
                .map_err(store_err)?;
            None
        }
        Kind::Tool => {
            let tools = adi_tools::Tools::with_config(config.clone());
            tools.archive(&element.id).map_err(store_err)?;
            tools.remove(&element.id).map_err(store_err)?;
            None
        }
        Kind::Trigger => {
            adi_triggers::Triggers::with_config(config.clone())
                .delete(&element.id)
                .map_err(store_err)?;
            None
        }
        Kind::Llm => {
            adi_agents::llm::LlmBackends::with_config(config.clone())
                .delete(&element.id)
                .map_err(store_err)?;
            None
        }
        Kind::Embedding => {
            uninstall_embedding(config, &element.id)?;
            None
        }
        Kind::Project => {
            adi_projects::Projects::with_config(config.clone())
                .remove(&element.id)
                .map_err(store_err)?;
            None
        }
        Kind::Dashboard => {
            uninstall_dashboard(&market.dashboards_dir(), &element.id)?;
            None
        }
        Kind::Service => uninstall_service(market, &addr.marketplace, &addr.slug, &ledger, &element)?,
    };

    ledger.elements.remove(idx);
    let bundle_removed = ledger.elements.is_empty();
    if bundle_removed {
        delete_ledger(market, &addr.marketplace, &addr.slug);
        let _ = std::fs::remove_dir_all(bundle_dir(market, &addr.marketplace, &addr.slug));
        let _ = std::fs::remove_dir_all(services_dir(market, &addr.marketplace, &addr.slug));
    } else {
        write_ledger(market, &addr.marketplace, &addr.slug, &ledger)?;
    }

    Ok(ElementUninstalled {
        marketplace: addr.marketplace,
        slug: addr.slug,
        kind: element.kind,
        name: element.name,
        id: element.id,
        bundle_removed,
        note,
    })
}

/// Start a parked hive service: copy its `ServiceSpec` block out of `marketplace/services/…` into
/// the target `hive.yaml` — the bundle's own project, when it carries one, else the global hive —
/// under its landed key, and drop the parked file. From here the supervisor's own periodic re-read
/// picks it up and its `start:` policy governs it like any hand-written service
/// (`docs/marketplace-bundles.md`, "Inert on arrival, per kind").
///
/// Idempotent: starting a service that is already started (its parked file is already gone, and its
/// block is already in the target hive.yaml) answers the same outcome rather than erroring.
///
/// # Errors
/// [`Error::BadAddress`] when `spec` does not name one `service` element, [`Error::BundleNotInstalled`]
/// / [`Error::ElementNotInstalled`] when the address does not name an installed service, and
/// [`Error::Store`] when the target hive.yaml cannot be read or written, or already names something
/// under this service's key.
pub fn start_service(market: &Marketplace, spec: &str) -> Result<ServiceStarted> {
    let addr = Address::parse(spec)?;
    let Some(target) = &addr.element else {
        return Err(Error::BadAddress(spec.to_string()));
    };
    if target.kind != Kind::Service {
        return Err(Error::BadAddress(spec.to_string()));
    }
    let name = target.name.clone().unwrap_or_default();
    let ledger = read_ledger(market, &addr.marketplace, &addr.slug)
        .ok_or_else(|| Error::BundleNotInstalled(addr.marketplace.clone(), addr.slug.clone()))?;
    let Some(ledger_el) = ledger.find(Kind::Service, &name) else {
        return Err(Error::ElementNotInstalled(spec.to_string()));
    };
    let id = ledger_el.id.clone();
    let project = ledger.project_id();
    let projects = adi_projects::Projects::with_config(market.config().clone());
    let target_hive = target_hive_path(market, &projects, project.as_deref())?;
    let parked_path = services_dir(market, &addr.marketplace, &addr.slug).join(format!("{id}.yaml"));

    if !parked_path.is_file() {
        // Either already started, or nothing was ever parked here — the two look identical from
        // this side, and only the first is a state worth answering rather than refusing.
        if read_service_block(&target_hive, &id).is_some() {
            return Ok(ServiceStarted {
                marketplace: addr.marketplace,
                slug: addr.slug,
                name,
                id,
                project,
            });
        }
        return Err(Error::ElementNotInstalled(spec.to_string()));
    }

    if read_service_block(&target_hive, &id).is_some() {
        return Err(Error::Store(format!(
            "{id} already names a service in {} — remove or rename it there first",
            target_hive.display()
        )));
    }
    let text = std::fs::read_to_string(&parked_path)?;
    write_service_block(&target_hive, &id, &text)?;
    let _ = std::fs::remove_file(&parked_path);

    Ok(ServiceStarted {
        marketplace: addr.marketplace,
        slug: addr.slug,
        name,
        id,
        project,
    })
}

/// One general bundle's status for a listing, per `docs/marketplace-bundles.md`'s "What
/// 'installed' means for a partially-installed bundle": installed is a fraction of what the
/// manifest's own preview declares, and every field here is computed live off the ledger, the
/// current pin and the current secrets store — never stored and frozen at install time, the same
/// posture `outdated` and `missing_secrets` already take right after an install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleStatus {
    /// How many elements the manifest's own preview lists — `0` when it publishes none at all,
    /// which is legal and simply means the listing has no denominator to show.
    pub declared: usize,
    /// Every element actually installed here, in ledger order.
    pub installed: Vec<LedgerElement>,
    /// Whether the ledger's own internal clone stands behind the manifest's current pin.
    pub outdated: bool,
    /// Every secret name an installed element still declares (an agent's `secrets`, a backend's
    /// `api_key_env`) that this machine does not currently have set, read fresh off the landed
    /// files and the secrets store on every call.
    pub missing_secrets: Vec<String>,
    /// Every element this bundle offers, whether or not it is installed here: the union of what
    /// the manifest's own preview declares and what the ledger actually landed, one row per
    /// `(kind, published name)`, in a fixed order ([`Kind::ALL`], then name) so a listing groups
    /// the same way for every bundle. This is what a row renders — the fraction above is the
    /// summary, this is the detail it summarizes.
    pub elements: Vec<BundleElementRow>,
}

/// One row of [`BundleStatus::elements`] — a declared element, an installed one, or both at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleElementRow {
    pub kind: Kind,
    /// The published name — the file or directory stem the repository carries it under, and the
    /// third address coordinate.
    pub name: String,
    /// The one line the manifest's preview publishes for it, when it publishes one at all — the
    /// preview is advisory, so this is absent whenever the manifest never described this element
    /// (an old-shape publisher, or one whose preview undersells what the tree actually carries).
    pub description: Option<String>,
    /// The id it landed as, or `None` when the manifest declares it but nothing has installed it
    /// here yet.
    pub id: Option<String>,
}

/// A general bundle's status for the listing — `None` only when there is truly nothing to say:
/// the manifest publishes no preview *and* nothing has been installed from it, which is exactly
/// what the legacy single-dashboard bundle looks like from here too (it keeps no ledger at all,
/// decision #6, and is shown through `crate::install::cached_apps` instead). A bundle that
/// declares a preview but has never been installed still answers `Some`, every row's `id` absent
/// — the preview is the only list there is to show before anything is cloned
/// (`docs/marketplace-bundles.md` decision #2), and showing nothing for it would read as "this
/// bundle carries nothing" when the honest answer is "nothing here has been installed yet."
#[must_use]
pub fn status(market: &Marketplace, marketplace: &str, slug: &str, entry: &BundleEntry) -> Option<BundleStatus> {
    let declared = entry.elements();
    let ledger = read_ledger(market, marketplace, slug);
    if declared.is_empty() && ledger.is_none() {
        return None;
    }

    let (installed, outdated, missing_secrets) = match &ledger {
        Some(ledger) => (
            ledger.elements.clone(),
            !ledger.commit.eq_ignore_ascii_case(&entry.pin()),
            live_missing_secrets(market.config(), ledger),
        ),
        None => (Vec::new(), false, Vec::new()),
    };

    // Every installed element first, carrying whatever description the preview publishes for it
    // when the two agree on kind and name — then every declared element nothing has installed
    // yet, `id: None`. A declared element the ledger already carries is never listed twice.
    //
    // The project scaffold is the one exception to "agree on kind and name": it lands under the
    // bundle's own slug, never under whatever name the manifest's preview happened to publish for
    // it (`docs/marketplace-bundles.md` decision #4 — it has no name of its own to land under, and
    // `install`'s own `addr.slug.clone()` is what actually lands). Matching it by name would never
    // agree with what landed and would draw the one scaffold twice: once installed, once still
    // "declared." So a project entry matches by kind alone, on both sides of this merge.
    let mut elements: Vec<BundleElementRow> = installed
        .iter()
        .map(|e| {
            let description = declared
                .iter()
                .find(|(kind, el)| *kind == e.kind && (e.kind == Kind::Project || el.name() == e.name))
                .and_then(|(_, el)| el.description().map(str::to_string));
            BundleElementRow {
                kind: e.kind,
                name: e.name.clone(),
                description,
                id: Some(e.id.clone()),
            }
        })
        .collect();
    for (kind, el) in &declared {
        let name = el.name();
        let already_listed = elements
            .iter()
            .any(|row| row.kind == *kind && (*kind == Kind::Project || row.name == name));
        if already_listed {
            continue;
        }
        elements.push(BundleElementRow {
            kind: *kind,
            // Predicts the name it would actually land under, rather than showing the preview's
            // own (irrelevant) name for a kind an address never names in the first place.
            name: if *kind == Kind::Project { slug.to_string() } else { name.to_string() },
            description: el.description().map(str::to_string),
            id: None,
        });
    }
    elements.sort_by_key(|row| {
        (
            Kind::ALL.iter().position(|k| *k == row.kind).unwrap_or(Kind::ALL.len()),
            row.name.clone(),
        )
    });

    Some(BundleStatus {
        declared: declared.len(),
        installed,
        outdated,
        missing_secrets,
        elements,
    })
}

/// Every declared secret name an installed element still lacks, read fresh rather than off
/// whatever was reported the moment it landed: an operator who sets a secret an hour later should
/// see the listing agree without reinstalling anything, and one who edits an agent's own
/// attachments in the panel should see *that* edit reflected too.
fn live_missing_secrets(config: &adi_config::Config, ledger: &Ledger) -> Vec<String> {
    let secrets = adi_secrets::Secrets::with_config(config.clone());
    let mut missing = BTreeSet::new();
    for el in &ledger.elements {
        match el.kind {
            Kind::Agent => {
                if let Ok(Some(agent)) = adi_agents::Agents::with_config(config.clone()).get(&el.id) {
                    for attachment in &agent.manifest.secrets {
                        if !secret_set(&secrets, attachment.project.as_deref(), &attachment.name) {
                            missing.insert(attachment.name.clone());
                        }
                    }
                }
            }
            Kind::Llm => {
                if let Ok(Some(backend)) =
                    adi_agents::llm::LlmBackends::with_config(config.clone()).get(&el.id)
                    && let Some(name) = backend.manifest.api_key_env.as_deref()
                    && !secret_set(&secrets, None, name)
                {
                    missing.insert(name.to_string());
                }
            }
            Kind::Embedding => {
                if let Ok(Some(backend)) =
                    adi_embeddings::EmbeddingBackends::with_config(config.clone()).get(&el.id)
                    && let Some(name) = backend.manifest.api_key_env.as_deref()
                    && !secret_set(&secrets, None, name)
                {
                    missing.insert(name.to_string());
                }
            }
            Kind::Tool | Kind::Dashboard | Kind::Service | Kind::Trigger | Kind::Project => {}
        }
    }
    missing.into_iter().collect()
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

    /// Dispatch one already-installed element to its kind's own update path — [`update`]'s
    /// per-element fast-forward, drift check included.
    fn reland(
        &self,
        ledger_el: &LedgerElement,
        layout_el: &LayoutElement,
        root: &Path,
        project: Option<&str>,
        target_hive: &Path,
        forced: bool,
    ) -> Relanded {
        match ledger_el.kind {
            Kind::Agent => reland_agent(&self.agents, root, layout_el, ledger_el, project, forced),
            Kind::Tool => reland_tool(&self.tools, root, layout_el, ledger_el, forced),
            Kind::Dashboard => reland_dashboard(&self.dashboards_dir, root, layout_el, ledger_el, forced),
            Kind::Llm => reland_llm(&self.llm_backends, root, layout_el, ledger_el, forced),
            Kind::Embedding => reland_embedding(&self.embedding_backends, root, layout_el, ledger_el, forced),
            Kind::Service => {
                reland_service(&self.parked_services_dir, target_hive, root, layout_el, ledger_el, forced)
            }
            Kind::Trigger => reland_trigger(&self.triggers, root, layout_el, ledger_el, project, forced),
            // No public store API rewrites a project's name or description in place outside
            // creation (`adi_projects::Projects` has `archive`/`unarchive`/`rename`/`remove` and
            // nothing else) — reported rather than silently skipped, and never relanded, forced
            // or not (`docs/marketplace-bundles.md`'s own "Update" section names this a flat-file
            // fast-forward like any other; the real store gives it no such path).
            Kind::Project => Relanded::Skipped(
                "a project scaffold's name and description cannot be changed after install — \
                 there is no in-place update for it in adi_projects — left as it stands"
                    .to_string(),
            ),
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

/// Fold one element's [`Relanded`] outcome into what [`update`] reports and — on success — the
/// fresh fingerprint the ledger should keep from here on.
fn fold_update(kind: Kind, name: String, id: String, relanded: Relanded) -> (ElementUpdateOutcome, Option<String>) {
    match relanded {
        Relanded::Ok {
            fingerprint,
            changed,
            note,
        } => (
            ElementUpdateOutcome {
                kind,
                name,
                id,
                changed,
                note,
            },
            Some(fingerprint),
        ),
        Relanded::Skipped(reason) => (
            ElementUpdateOutcome {
                kind,
                name,
                id,
                changed: false,
                note: Some(reason),
            },
            None,
        ),
        Relanded::Failed(reason) => (
            ElementUpdateOutcome {
                kind,
                name,
                id,
                changed: false,
                note: Some(format!("did not update: {reason}")),
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

/// Drop a bundle's ledger file — [`uninstall_element`]'s last step once nothing it names remains
/// installed. Best-effort: a ledger that is already gone is not this function's problem.
fn delete_ledger(market: &Marketplace, marketplace: &str, slug: &str) {
    let path = market
        .config()
        .module(crate::MODULE)
        .raw_path(&ledger_raw_name(marketplace, slug));
    let _ = std::fs::remove_file(path);
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

// MARK: update — the note shared by every relanding kind's drift check

/// What every `reland_*` function answers when the live fingerprint has drifted from the ledger's
/// and `force` did not name this element — v1's `Dirty` refusal, scoped to one element.
fn drift_note() -> String {
    "edited since it was installed or last updated — left alone; update this element with --force \
     to overwrite the edit and lose it"
        .to_string()
}

/// The note a successful reland carries when it overwrote a drifted element anyway, because
/// `force` named it — `None` when there was nothing to overwrite in the first place.
fn forced_note(was_dirty: bool) -> Option<String> {
    was_dirty.then(|| "forced over a local edit — the edit is gone".to_string())
}

// MARK: hive service blocks — one key inside a hive.yaml's `services:` mapping, read generically
// so every field this crate does not model survives (the same reasoning `create_service` in
// `adi-webapp-api` already applies to the same file).

/// The raw YAML of `services.<key>` in the hive.yaml at `path`, or `None` when the file, the
/// mapping, or the key itself is not there.
fn read_service_block(path: &Path, key: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&raw).ok()?;
    let value = doc.get("services")?.as_mapping()?.get(key)?;
    serde_yaml_ng::to_string(value).ok()
}

/// Insert or overwrite `services.<key>` in the hive.yaml at `path` with `block_yaml`, creating the
/// file (and the `services:` mapping) if neither exists yet, and leaving every other key exactly as
/// it was.
///
/// # Errors
/// [`Error::Store`] when the existing file, once read, is not a YAML mapping, or `block_yaml`
/// itself does not parse; [`Error::Io`] on a read or write failure.
fn write_service_block(path: &Path, key: &str, block_yaml: &str) -> Result<()> {
    use serde_yaml_ng::{Mapping, Value as Yaml};

    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: Yaml = if raw.trim().is_empty() {
        Yaml::Mapping(Mapping::new())
    } else {
        serde_yaml_ng::from_str(&raw)
            .map_err(|e| Error::Store(format!("parsing {}: {e}", path.display())))?
    };
    let value: Yaml = serde_yaml_ng::from_str(block_yaml)
        .map_err(|e| Error::Store(format!("re-parsing the service before writing it: {e}")))?;
    let Yaml::Mapping(root) = &mut doc else {
        return Err(Error::Store(format!("{} is not a YAML mapping", path.display())));
    };
    let services = root
        .entry(Yaml::String("services".to_string()))
        .or_insert_with(|| Yaml::Mapping(Mapping::new()));
    let Yaml::Mapping(services) = services else {
        return Err(Error::Store(format!("{}'s services is not a mapping", path.display())));
    };
    services.insert(Yaml::String(key.to_string()), value);

    let text = serde_yaml_ng::to_string(&doc)
        .map_err(|e| Error::Store(format!("encoding {}: {e}", path.display())))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// Remove `services.<key>` from the hive.yaml at `path`. Answers the raw YAML of what was removed
/// (which is what a caller reads to decide whether it was a `runner.docker` service worth a note),
/// or `None` when the file, the mapping, or the key was never there.
///
/// # Errors
/// [`Error::Store`] when the file, once read, is not a YAML mapping; [`Error::Io`] on a write
/// failure.
fn drop_service_block(path: &Path, key: &str) -> Result<Option<String>> {
    use serde_yaml_ng::Value as Yaml;

    let Ok(raw) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let mut doc: Yaml = serde_yaml_ng::from_str(&raw)
        .map_err(|e| Error::Store(format!("parsing {}: {e}", path.display())))?;
    let Yaml::Mapping(root) = &mut doc else {
        return Ok(None);
    };
    let Some(Yaml::Mapping(services)) = root.get_mut("services") else {
        return Ok(None);
    };
    let Some(removed) = services.remove(key) else {
        return Ok(None);
    };
    let text = serde_yaml_ng::to_string(&doc)
        .map_err(|e| Error::Store(format!("encoding {}: {e}", path.display())))?;
    std::fs::write(path, text)?;
    Ok(Some(serde_yaml_ng::to_string(&removed).unwrap_or_default()))
}

/// Where a bundle's hive service lands once started, and where its update or its uninstall looks
/// for it having been: the bundle's own project's `.adi/hive.yaml` when it carries one, else the
/// global `hive.yaml` — the same project-scoped escape hatch every other kind's naming collision
/// gets (`docs/marketplace-bundles.md` decision #4).
fn target_hive_path(
    market: &Marketplace,
    projects: &adi_projects::Projects,
    project: Option<&str>,
) -> Result<PathBuf> {
    match project {
        Some(id) => projects.hive_path(id).map_err(|e| Error::Store(e.to_string())),
        None => Ok(market.config().module("hive").raw_path("hive.yaml")),
    }
}

/// Whether a service block names a `runner.docker` container — the note [`uninstall_service`]
/// attaches, because dropping the key stops the supervisor's own `docker wait` loop but never the
/// container itself (`docs/marketplace-bundles.md`, "Inert on arrival, per kind").
fn docker_note(block_yaml: &str) -> Option<String> {
    let spec: adi_hive::config::ServiceSpec = serde_yaml_ng::from_str(block_yaml).ok()?;
    spec.runner
        .as_ref()
        .is_some_and(|r| r.docker.is_some())
        .then(|| {
            "this was a runner.docker service — the supervisor no longer knows about it, but the \
             container itself is documented to survive that; stop and remove it yourself with \
             `docker stop`/`docker rm`"
                .to_string()
        })
}

// MARK: uninstall — one element, by its own kind's ordinary path

/// Wrap any per-kind store's own error as [`Error::Store`] — every one of these crates already
/// phrases its own refusals for the person reading them, so nothing here re-translates.
fn store_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Store(e.to_string())
}

/// Uninstall an embedding backend — refused while a consumer still resolves through it, the same
/// check `POST /api/embeddings/backends/delete` makes (`crates/adi-webapp-api/src/handlers/
/// embedding_backends.rs`), reproduced here because this call never goes through that endpoint.
///
/// # Errors
/// [`Error::EmbeddingInUse`] while a consumer still resolves through it, [`Error::Store`] for
/// anything else the registry refuses.
fn uninstall_embedding(config: &adi_config::Config, id: &str) -> Result<()> {
    let settings = adi_embeddings::EmbeddingSettings::open(config).map_err(store_err)?;
    let users: Vec<String> = [
        adi_embeddings::CONSUMER_INDEXER,
        adi_embeddings::CONSUMER_KNOWLEDGE,
        adi_embeddings::CONSUMER_FACTS,
    ]
    .into_iter()
    .filter(|consumer| settings.assignments.get(*consumer).map(String::as_str) == Some(id))
    .map(str::to_string)
    .collect();
    if !users.is_empty() {
        return Err(Error::EmbeddingInUse(format!(
            "{id} is still assigned to {} — point {} at another backend first",
            users.join(", "),
            if users.len() == 1 { "it" } else { "them" }
        )));
    }
    adi_embeddings::EmbeddingBackends::with_config(config.clone())
        .delete(id)
        .map_err(store_err)?;
    Ok(())
}

/// Uninstall a dashboard — the Dashboards page's own Archive → Delete, in one call: stamp
/// `archived_at`, park its hive file so the supervisor's glob no longer matches it, then remove the
/// directory whole. A directory that is already gone is not an error — nothing left to remove.
///
/// # Errors
/// [`Error::Io`] on a write or removal failure.
fn uninstall_dashboard(dashboards_dir: &Path, id: &str) -> Result<()> {
    let dest = dashboards_dir.join(id);
    if !dest.is_dir() {
        return Ok(());
    }
    let mut manifest = adi_dashboards::read_manifest(&dest);
    manifest.archived_at = Some(adi_config::now_unix());
    adi_dashboards::write_manifest(&dest, &manifest)?;
    let _ = std::fs::remove_file(dest.join(".adi").join(adi_dashboards::HIVE_LIVE));
    std::fs::remove_dir_all(&dest)?;
    Ok(())
}

/// Uninstall a hive service: drop it from wherever it is — the parked fragment file if it was never
/// started, the target hive.yaml's `services` mapping if it was — and say so when it was a
/// `runner.docker` service whose container survives regardless.
///
/// # Errors
/// [`Error::Store`] when the target hive.yaml cannot be read as YAML; [`Error::Io`] on a write
/// failure.
fn uninstall_service(
    market: &Marketplace,
    marketplace: &str,
    slug: &str,
    ledger: &Ledger,
    element: &LedgerElement,
) -> Result<Option<String>> {
    let parked_path = services_dir(market, marketplace, slug).join(format!("{}.yaml", element.id));
    if parked_path.is_file() {
        let text = std::fs::read_to_string(&parked_path).unwrap_or_default();
        let _ = std::fs::remove_file(&parked_path);
        return Ok(docker_note(&text));
    }
    let projects = adi_projects::Projects::with_config(market.config().clone());
    let target = target_hive_path(market, &projects, ledger.project_id().as_deref())?;
    match drop_service_block(&target, &element.id)? {
        Some(text) => Ok(docker_note(&text)),
        None => Ok(Some(
            "was not found parked or in its hive.yaml — nothing left to remove there, but it is \
             dropped from this bundle's own record"
                .to_string(),
        )),
    }
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

/// `update`'s own fast-forward of an already-landed agent — same file, same id, no minting: a
/// collision was already settled the first time this element landed.
fn reland_agent(
    agents: &adi_agents::Agents,
    root: &Path,
    layout_el: &LayoutElement,
    ledger_el: &LedgerElement,
    project: Option<&str>,
    forced: bool,
) -> Relanded {
    let path = agents.dir().join(format!("{}.toml", ledger_el.id));
    let live_fp = fingerprint_file(&path);
    if live_fp != ledger_el.fingerprint && !forced {
        return Relanded::Skipped(drift_note());
    }
    let text = match std::fs::read_to_string(root.join(&layout_el.path)) {
        Ok(t) => t,
        Err(e) => return Relanded::Failed(e.to_string()),
    };
    let mut manifest: adi_agents::StoredAgentManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Relanded::Failed(format!("agents/{}.toml: {e}", ledger_el.name)),
    };
    manifest.created_at = 0;
    manifest.updated_at = 0;
    manifest.project = project.map(str::to_string);
    match agents.save(&ledger_el.id, manifest) {
        Ok(_) => {
            let fingerprint = fingerprint_file(&path);
            Relanded::Ok {
                changed: fingerprint != ledger_el.fingerprint,
                note: forced_note(live_fp != ledger_el.fingerprint),
                fingerprint,
            }
        }
        Err(e) => Relanded::Failed(e.to_string()),
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

/// `update`'s own fast-forward of an already-landed trigger — forced disabled again on every
/// update, exactly as on a fresh install: the repository's `enabled` is never the operator's
/// deliberate act, whatever pin it arrives on.
fn reland_trigger(
    triggers: &adi_triggers::Triggers,
    root: &Path,
    layout_el: &LayoutElement,
    ledger_el: &LedgerElement,
    project: Option<&str>,
    forced: bool,
) -> Relanded {
    let path = triggers.dir().join(format!("{}.toml", ledger_el.id));
    let live_fp = fingerprint_file(&path);
    if live_fp != ledger_el.fingerprint && !forced {
        return Relanded::Skipped(drift_note());
    }
    let text = match std::fs::read_to_string(root.join(&layout_el.path)) {
        Ok(t) => t,
        Err(e) => return Relanded::Failed(e.to_string()),
    };
    let mut manifest: adi_triggers::TriggerManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Relanded::Failed(format!("triggers/{}.toml: {e}", ledger_el.name)),
    };
    manifest.created_at = 0;
    manifest.updated_at = 0;
    manifest.project = project.map(str::to_string);
    let mut note = forced_note(live_fp != ledger_el.fingerprint);
    if manifest.enabled {
        let forced_disabled = format!(
            "triggers/{}.toml asked to arrive enabled — forced to disabled again",
            ledger_el.name
        );
        note = Some(match note {
            Some(existing) => format!("{existing}; {forced_disabled}"),
            None => forced_disabled,
        });
    }
    manifest.enabled = false;
    match triggers.save(&ledger_el.id, manifest) {
        Ok(_) => {
            let fingerprint = fingerprint_file(&path);
            Relanded::Ok {
                changed: fingerprint != ledger_el.fingerprint,
                fingerprint,
                note,
            }
        }
        Err(e) => Relanded::Failed(e.to_string()),
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

/// `update`'s own fast-forward of an already-landed dashboard's tracked files — the same carve-out
/// v1's own update already gives the three generated entry points (`GENERATED_DASHBOARD_FILES`):
/// they are the panel's to rewrite, so they are never collected into what gets written here, and
/// never counted as an edit worth blocking the rest of the dashboard's own fast-forward.
fn reland_dashboard(
    dashboards_dir: &Path,
    root: &Path,
    layout_el: &LayoutElement,
    ledger_el: &LedgerElement,
    forced: bool,
) -> Relanded {
    let dest = dashboards_dir.join(&ledger_el.id);
    let mut live_files = Vec::new();
    let mut live_rel = PathBuf::new();
    let mut live_total = 0_u64;
    if let Err(e) = adi_dashboards::collect_files(&dest, &mut live_rel, &mut live_files, &mut live_total) {
        return Relanded::Failed(e.to_string());
    }
    let live_fp = fingerprint_dashboard_files(&live_files);
    if live_fp != ledger_el.fingerprint && !forced {
        return Relanded::Skipped(drift_note());
    }

    let src = root.join(&layout_el.path);
    let mut new_files = Vec::new();
    let mut new_rel = PathBuf::new();
    let mut new_total = 0_u64;
    if let Err(e) = adi_dashboards::collect_files(&src, &mut new_rel, &mut new_files, &mut new_total) {
        return Relanded::Failed(e.to_string());
    }
    let writable: Vec<adi_dashboards::BundleFile> = new_files
        .iter()
        .filter(|f| !GENERATED_DASHBOARD_FILES.contains(&f.path.as_str()))
        .cloned()
        .collect();
    let decoded = match adi_dashboards::decode_bundle(&dest, &writable) {
        Ok(d) => d,
        Err(e) => return Relanded::Failed(e.to_string()),
    };
    for (path, bytes) in decoded {
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Relanded::Failed(e.to_string());
        }
        if let Err(e) = std::fs::write(&path, bytes) {
            return Relanded::Failed(e.to_string());
        }
    }

    let fingerprint = fingerprint_dashboard_files(&new_files);
    Relanded::Ok {
        changed: fingerprint != ledger_el.fingerprint,
        note: forced_note(live_fp != ledger_el.fingerprint),
        fingerprint,
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

/// `update`'s own fast-forward of an already-landed hive service — the parked fragment file if it
/// was never started, or the block inside `target_hive` if it was (`docs/marketplace-bundles.md`,
/// "Update"): a started service's entry is a plain YAML block an operator could just as well have
/// hand-edited, so it gets the same drift check either way.
fn reland_service(
    parked_dir: &Path,
    target_hive: &Path,
    root: &Path,
    layout_el: &LayoutElement,
    ledger_el: &LedgerElement,
    forced: bool,
) -> Relanded {
    let text = match std::fs::read_to_string(root.join(&layout_el.path)) {
        Ok(t) => t,
        Err(e) => return Relanded::Failed(e.to_string()),
    };
    if let Err(e) = serde_yaml_ng::from_str::<adi_hive::config::ServiceSpec>(&text) {
        return Relanded::Failed(format!("services/{}.yaml is not a hive service: {e}", ledger_el.name));
    }

    let parked_path = parked_dir.join(format!("{}.yaml", ledger_el.id));
    if parked_path.is_file() {
        let live_fp = fingerprint_file(&parked_path);
        if live_fp != ledger_el.fingerprint && !forced {
            return Relanded::Skipped(drift_note());
        }
        if let Err(e) = std::fs::write(&parked_path, &text) {
            return Relanded::Failed(e.to_string());
        }
        let fingerprint = fingerprint_bytes(text.as_bytes());
        return Relanded::Ok {
            changed: fingerprint != ledger_el.fingerprint,
            note: forced_note(live_fp != ledger_el.fingerprint),
            fingerprint,
        };
    }

    let Some(live_block) = read_service_block(target_hive, &ledger_el.id) else {
        return Relanded::Skipped("no longer found parked or in its hive.yaml — left alone".to_string());
    };
    let live_fp = fingerprint_bytes(live_block.as_bytes());
    if live_fp != ledger_el.fingerprint && !forced {
        return Relanded::Skipped(drift_note());
    }
    if let Err(e) = write_service_block(target_hive, &ledger_el.id, &text) {
        return Relanded::Failed(e.to_string());
    }
    let fingerprint = fingerprint_bytes(text.as_bytes());
    Relanded::Ok {
        changed: fingerprint != ledger_el.fingerprint,
        note: forced_note(live_fp != ledger_el.fingerprint),
        fingerprint,
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

/// `update`'s own fast-forward of an already-landed tool: only the script content ever came from
/// the repository (`config.toml`'s name and runtime are this machine's own, set once at install),
/// so that is all a fast-forward ever rewrites.
fn reland_tool(
    tools: &adi_tools::Tools,
    root: &Path,
    layout_el: &LayoutElement,
    ledger_el: &LedgerElement,
    forced: bool,
) -> Relanded {
    let cfg_path = tools.tool_dir(&ledger_el.id).unwrap_or_default().join("config.toml");
    let script_path = tools.script_path(&ledger_el.id).unwrap_or_default();
    let live_fp = fingerprint_files(&[cfg_path.clone(), script_path.clone()]);
    if live_fp != ledger_el.fingerprint && !forced {
        return Relanded::Skipped(drift_note());
    }
    let content = match std::fs::read_to_string(root.join(&layout_el.path)) {
        Ok(c) => c,
        Err(e) => return Relanded::Failed(e.to_string()),
    };
    if let Err(e) = tools.write_script(&ledger_el.id, &content) {
        return Relanded::Failed(e.to_string());
    }
    let fingerprint = fingerprint_files(&[cfg_path, script_path]);
    Relanded::Ok {
        changed: fingerprint != ledger_el.fingerprint,
        note: forced_note(live_fp != ledger_el.fingerprint),
        fingerprint,
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

/// `update`'s own fast-forward of an already-landed LLM backend — always verbatim, since this kind
/// never mints past a collision (its id is always its published name).
fn reland_llm(
    backends: &adi_agents::llm::LlmBackends,
    root: &Path,
    layout_el: &LayoutElement,
    ledger_el: &LedgerElement,
    forced: bool,
) -> Relanded {
    let path = backends.dir().join(format!("{}.toml", ledger_el.id));
    let live_fp = fingerprint_file(&path);
    if live_fp != ledger_el.fingerprint && !forced {
        return Relanded::Skipped(drift_note());
    }
    let text = match std::fs::read_to_string(root.join(&layout_el.path)) {
        Ok(t) => t,
        Err(e) => return Relanded::Failed(e.to_string()),
    };
    let manifest: adi_agents::llm::LlmBackendManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Relanded::Failed(format!("llm/{}.toml: {e}", ledger_el.name)),
    };
    match backends.save(&ledger_el.id, manifest) {
        Ok(_) => {
            let fingerprint = fingerprint_file(&path);
            Relanded::Ok {
                changed: fingerprint != ledger_el.fingerprint,
                note: forced_note(live_fp != ledger_el.fingerprint),
                fingerprint,
            }
        }
        Err(e) => Relanded::Failed(e.to_string()),
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

/// `update`'s own fast-forward of an already-landed embedding backend — the same verbatim-only
/// reasoning as [`reland_llm`].
fn reland_embedding(
    backends: &adi_embeddings::EmbeddingBackends,
    root: &Path,
    layout_el: &LayoutElement,
    ledger_el: &LedgerElement,
    forced: bool,
) -> Relanded {
    let path = backends.dir().join(format!("{}.toml", ledger_el.id));
    let live_fp = fingerprint_file(&path);
    if live_fp != ledger_el.fingerprint && !forced {
        return Relanded::Skipped(drift_note());
    }
    let text = match std::fs::read_to_string(root.join(&layout_el.path)) {
        Ok(t) => t,
        Err(e) => return Relanded::Failed(e.to_string()),
    };
    let manifest: adi_embeddings::EmbeddingBackendManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => return Relanded::Failed(format!("embeddings/{}.toml: {e}", ledger_el.name)),
    };
    match backends.save(&ledger_el.id, manifest) {
        Ok(_) => {
            let fingerprint = fingerprint_file(&path);
            Relanded::Ok {
                changed: fingerprint != ledger_el.fingerprint,
                note: forced_note(live_fp != ledger_el.fingerprint),
                fingerprint,
            }
        }
        Err(e) => Relanded::Failed(e.to_string()),
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

    fn write_docker_service(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("services")).expect("dir");
        std::fs::write(
            root.join("services").join(format!("{name}.yaml")),
            "runner:\n  docker:\n    image: postgres:16\n",
        )
        .expect("service");
    }

    fn upstream_dir(market: &Marketplace) -> PathBuf {
        market.config().root().join("publisher").join("upstream")
    }

    /// Mutate the upstream repository and commit — the fixture's own stand-in for "the publisher
    /// cut a new release" — then re-sync `market` so its cached pin moves onto that new commit.
    fn advance(market: &Marketplace, slug: &str, repo: &str, mutate: impl FnOnce(&Path)) -> String {
        let root = upstream_dir(market);
        mutate(&root);
        let commit = git_commit(&root, "v2");
        synced(market, slug, repo, &commit);
        commit
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

        let installed = unwrap_bundle(install(&market, "adi/crm-suite", "", false).expect("install"));
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
        let installed = unwrap_bundle(install(&market, "adi/solo-agent", "", false).expect("install"));
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
        let installed = unwrap_bundle(install(&market, "adi/stamped-bundle", "", false).expect("install"));
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

        let installed = unwrap_bundle(install(&market, "adi/mint-bundle", "", false).expect("install"));
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

        let installed = unwrap_bundle(install(&market, "adi/tool-bundle", "", false).expect("install"));
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

        let installed = unwrap_bundle(install(&market, "adi/llm-bundle", "", false).expect("install"));
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

        let installed = unwrap_bundle(install(&market, "adi/embed-bundle", "", false).expect("install"));
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

        let installed = unwrap_bundle(install(&market, "adi/proj-bundle", "", false).expect("install"));
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
        let installed = unwrap_bundle(install(&market, "adi/secret-bundle", "", false).expect("install"));
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
        let installed = unwrap_bundle(install(&market, "adi/dangling-bundle/agents/sales-bot", "", false).expect("install"));
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
        let first = unwrap_bundle(install(&market, "adi/grow-bundle", "", false).expect("first"));
        let first_id = outcome_of(&first, Kind::Agent, "sales-bot").id.clone().expect("id");

        let second = unwrap_bundle(install(&market, "adi/grow-bundle", "", false).expect("second"));
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
        let installed = unwrap_bundle(install(&market, "adi/single-bundle/tools/csv-import", "", false).expect("install"));
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
        let err = install(&market, "adi/unknown-el-bundle/agents/nope", "", false).expect_err("refused");
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
        let err = install(&market, "adi/rust-bundle", "", false).expect_err("refused");
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

        let outcome = install(&market, "adi/crm", "", false).expect("install");
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

    // MARK: growing an install — phase C item 3

    #[test]
    fn installing_a_further_element_later_grows_the_same_ledger_entry() {
        let (market, ..) = fixture("grow-further", "grow-further-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_tool(root, "csv-import");
        });
        let first = unwrap_bundle(install(&market, "adi/grow-further-bundle/agents/sales-bot", "", false).expect("first"));
        assert_eq!(first.elements.len(), 1);
        let installed_at = read_ledger(&market, "adi", "grow-further-bundle").expect("ledger").installed_at;

        let second = unwrap_bundle(install(&market, "adi/grow-further-bundle/tools/csv-import", "", false).expect("second"));
        assert_eq!(second.elements.len(), 1);
        assert_eq!(second.elements[0].kind, Kind::Tool);

        let ledger = read_ledger(&market, "adi", "grow-further-bundle").expect("ledger");
        assert_eq!(ledger.elements.len(), 2, "the same entry grew rather than a second ledger appearing");
        assert_eq!(ledger.installed_at, installed_at, "still the original install's own record");
        assert!(
            adi_agents::Agents::with_config(market.config().clone())
                .get("sales-bot")
                .expect("get")
                .is_some(),
            "the first element is untouched by the second call"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    // MARK: update

    #[test]
    fn update_refuses_when_nothing_is_installed() {
        let (market, ..) = fixture("upd-none", "upd-none-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        let err = update(&market, "adi", "upd-none-bundle", &[]).expect_err("refused");
        assert!(matches!(err, Error::BundleNotInstalled(_, _)), "{err}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn updating_a_bundle_still_at_its_pin_is_a_no_op() {
        let (market, ..) = fixture("upd-noop", "upd-noop-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        unwrap_bundle(install(&market, "adi/upd-noop-bundle", "", false).expect("install"));
        let done = update(&market, "adi", "upd-noop-bundle", &[]).expect("update");
        assert!(!done.changed);
        assert_eq!(done.from, done.to);
        assert_eq!(done.elements.len(), 1);
        assert!(!done.elements[0].changed);
        assert!(done.elements[0].note.is_none());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_unedited_flat_file_element_fast_forwards_on_update() {
        let (market, repo, _first) = fixture("upd-ff", "upd-ff-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        let installed = unwrap_bundle(install(&market, "adi/upd-ff-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Agent, "sales-bot").id.clone().expect("id");

        advance(&market, "upd-ff-bundle", &repo, |root| {
            write_agent(root, "sales-bot", "starred = true\n");
        });
        let done = update(&market, "adi", "upd-ff-bundle", &[]).expect("update");
        assert!(done.changed);
        let outcome = done
            .elements
            .iter()
            .find(|e| e.kind == Kind::Agent && e.name == "sales-bot")
            .expect("outcome");
        assert!(outcome.changed);
        assert!(outcome.note.is_none());

        let agent = adi_agents::Agents::with_config(market.config().clone())
            .get(&id)
            .expect("get")
            .expect("present");
        assert!(agent.manifest.starred, "fast-forwarded onto the new pin's content");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn an_edited_flat_file_element_is_left_alone_and_reported() {
        let (market, repo, _first) = fixture("upd-dirty", "upd-dirty-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        let installed = unwrap_bundle(install(&market, "adi/upd-dirty-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Agent, "sales-bot").id.clone().expect("id");

        // The operator edits the installed agent, through the store's own path.
        let agents = adi_agents::Agents::with_config(market.config().clone());
        let mut manifest = agents.get(&id).expect("get").expect("present").manifest;
        manifest.starred = true;
        agents.save(&id, manifest).expect("operator edit");

        advance(&market, "upd-dirty-bundle", &repo, |root| {
            write_agent(root, "sales-bot", "starred = false\n");
        });
        let done = update(&market, "adi", "upd-dirty-bundle", &[]).expect("update");
        let outcome = done.elements.iter().find(|e| e.kind == Kind::Agent).expect("outcome");
        assert!(!outcome.changed);
        assert!(outcome.note.as_deref().is_some_and(|n| n.contains("edited")), "{outcome:?}");

        let agent = agents.get(&id).expect("get").expect("present");
        assert!(agent.manifest.starred, "left exactly as the operator set it");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn force_on_one_element_overwrites_and_loses_the_edit() {
        let (market, repo, _first) = fixture("upd-force", "upd-force-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        let installed = unwrap_bundle(install(&market, "adi/upd-force-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Agent, "sales-bot").id.clone().expect("id");
        let agents = adi_agents::Agents::with_config(market.config().clone());
        let mut manifest = agents.get(&id).expect("get").expect("present").manifest;
        manifest.starred = true;
        agents.save(&id, manifest).expect("operator edit");

        // Tagged, not just un-starred: a field the original install never carried, so the
        // fast-forwarded content can never coincidentally hash the same as what was there before
        // the operator's edit even happened (which `starred = false` alone would, `now_unix()`
        // being second-granular and this test running well inside one).
        advance(&market, "upd-force-bundle", &repo, |root| {
            write_agent(root, "sales-bot", "starred = false\ntags = [\"from-new-pin\"]\n");
        });
        let done = update(&market, "adi", "upd-force-bundle", &[(Kind::Agent, "sales-bot")]).expect("update");
        let outcome = done.elements.iter().find(|e| e.kind == Kind::Agent).expect("outcome");
        assert!(outcome.changed);
        assert!(outcome.note.as_deref().is_some_and(|n| n.contains("forced")), "{outcome:?}");

        let agent = agents.get(&id).expect("get").expect("present");
        assert!(!agent.manifest.starred, "the local edit is gone, overwritten by the forced update");
        assert_eq!(agent.manifest.tags, vec!["from-new-pin".to_string()]);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn partial_success_reports_which_elements_moved_and_which_were_blocked() {
        let (market, repo, _first) = fixture("upd-partial", "upd-partial-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_tool(root, "csv-import");
        });
        let installed = unwrap_bundle(install(&market, "adi/upd-partial-bundle", "", false).expect("install"));
        let agent_id = outcome_of(&installed, Kind::Agent, "sales-bot").id.clone().expect("id");

        let agents = adi_agents::Agents::with_config(market.config().clone());
        let mut manifest = agents.get(&agent_id).expect("get").expect("present").manifest;
        manifest.starred = true;
        agents.save(&agent_id, manifest).expect("operator edit");

        advance(&market, "upd-partial-bundle", &repo, |root| {
            write_agent(root, "sales-bot", "starred = false\n");
            std::fs::write(root.join("tools").join("csv-import.sh"), "#!/bin/sh\necho v2\n").expect("tool v2");
        });
        let done = update(&market, "adi", "upd-partial-bundle", &[]).expect("update");
        assert!(done.changed, "the clone itself moved");
        let agent_outcome = done.elements.iter().find(|e| e.kind == Kind::Agent).expect("agent");
        assert!(!agent_outcome.changed);
        assert!(agent_outcome.note.is_some());
        let tool_outcome = done.elements.iter().find(|e| e.kind == Kind::Tool).expect("tool");
        assert!(tool_outcome.changed);
        assert!(tool_outcome.note.is_none());

        let tools = adi_tools::Tools::with_config(market.config().clone());
        assert_eq!(tools.read_script("csv-import").expect("read"), "#!/bin/sh\necho v2\n");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_dashboards_generated_entry_points_are_excluded_from_the_drift_check() {
        let (market, repo, _first) = fixture("upd-dash", "upd-dash-bundle", |root| {
            write_dashboard(root, "crm");
            let modules = root.join("dashboards").join("crm").join("frontend").join("modules");
            std::fs::create_dir_all(&modules).expect("modules dir");
            std::fs::write(modules.join("panel.ts"), "// panel v1\n").expect("panel v1");
        });
        let installed = unwrap_bundle(install(&market, "adi/upd-dash-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Dashboard, "crm").id.clone().expect("id");
        let dest = market.dashboards_dir().join(&id);

        // The panel rewrites a generated entry point in place — never the operator's edit to
        // notice, and never something an update fast-forwards back over.
        std::fs::write(dest.join("frontend").join("index.ts"), "// panel rewrite\n").expect("panel");

        advance(&market, "upd-dash-bundle", &repo, |root| {
            std::fs::write(
                root.join("dashboards").join("crm").join("frontend").join("modules").join("panel.ts"),
                "// panel v2\n",
            )
            .expect("panel v2");
            std::fs::write(
                root.join("dashboards").join("crm").join("backend").join("index.ts"),
                "// back v2\n",
            )
            .expect("back v2 (generated — must never land)");
        });
        let done = update(&market, "adi", "upd-dash-bundle", &[]).expect("update");
        let outcome = done.elements.iter().find(|e| e.kind == Kind::Dashboard).expect("outcome");
        assert!(outcome.changed, "{outcome:?}");
        assert!(outcome.note.is_none(), "the panel's own rewrite must not read as an edit: {outcome:?}");

        assert_eq!(
            std::fs::read_to_string(dest.join("frontend").join("modules").join("panel.ts")).expect("panel"),
            "// panel v2\n",
            "an ordinary tracked file is fast-forwarded"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("frontend").join("index.ts")).expect("front"),
            "// panel rewrite\n",
            "the generated entry point is never fast-forwarded over"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("backend").join("index.ts")).expect("back"),
            "// back\n",
            "the other generated entry point is untouched by the update too"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_hive_service_still_parked_updates_the_parked_fragment() {
        let (market, repo, _first) = fixture("upd-svc-parked", "upd-svc-parked-bundle", |root| {
            write_service(root, "redis");
        });
        unwrap_bundle(install(&market, "adi/upd-svc-parked-bundle", "", false).expect("install"));
        advance(&market, "upd-svc-parked-bundle", &repo, |root| {
            std::fs::write(root.join("services").join("redis.yaml"), "proxy:\n  host: redis2.adi\n")
                .expect("v2");
        });
        let done = update(&market, "adi", "upd-svc-parked-bundle", &[]).expect("update");
        let outcome = done.elements.iter().find(|e| e.kind == Kind::Service).expect("outcome");
        assert!(outcome.changed);
        let parked = services_dir(&market, "adi", "upd-svc-parked-bundle").join(format!("{}.yaml", outcome.id));
        assert!(std::fs::read_to_string(parked).expect("parked").contains("redis2.adi"));
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_started_hive_services_live_entry_updates_in_its_hive_yaml() {
        let (market, repo, _first) = fixture("upd-svc-started", "upd-svc-started-bundle", |root| {
            write_service(root, "redis");
        });
        let installed = unwrap_bundle(install(&market, "adi/upd-svc-started-bundle", "", false).expect("install"));
        let svc_id = outcome_of(&installed, Kind::Service, "redis").id.clone().expect("id");
        start_service(&market, "adi/upd-svc-started-bundle/services/redis").expect("start");
        let global_hive = market.config().module("hive").raw_path("hive.yaml");
        assert!(read_service_block(&global_hive, &svc_id).is_some());

        advance(&market, "upd-svc-started-bundle", &repo, |root| {
            std::fs::write(root.join("services").join("redis.yaml"), "proxy:\n  host: redis2.adi\n")
                .expect("v2");
        });
        let done = update(&market, "adi", "upd-svc-started-bundle", &[]).expect("update");
        let outcome = done.elements.iter().find(|e| e.kind == Kind::Service).expect("outcome");
        assert!(outcome.changed, "{outcome:?}");
        let block = read_service_block(&global_hive, &svc_id).expect("block");
        assert!(block.contains("redis2.adi"), "{block}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn the_legacy_bundle_has_no_ledger_so_update_refuses_cleanly() {
        let (market, ..) = fixture("upd-legacy", "crm", |root| {
            std::fs::create_dir_all(root.join("frontend")).expect("frontend");
            std::fs::create_dir_all(root.join("backend")).expect("backend");
            std::fs::write(root.join("frontend").join("index.ts"), "// front\n").expect("f");
            std::fs::write(root.join("backend").join("index.ts"), "// back\n").expect("b");
        });
        install(&market, "adi/crm", "", false).expect("install");
        let err = update(&market, "adi", "crm", &[]).expect_err("refused");
        assert!(matches!(err, Error::BundleNotInstalled(_, _)), "{err}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    // MARK: uninstall

    #[test]
    fn uninstalling_one_element_leaves_siblings_and_the_ledger_untouched() {
        let (market, ..) = fixture("uninst-sib", "uninst-sib-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_tool(root, "csv-import");
        });
        unwrap_bundle(install(&market, "adi/uninst-sib-bundle", "", false).expect("install"));
        let done = uninstall_element(&market, "adi/uninst-sib-bundle/agents/sales-bot").expect("uninstall");
        assert!(!done.bundle_removed);
        assert!(
            adi_agents::Agents::with_config(market.config().clone())
                .get("sales-bot")
                .expect("get")
                .is_none()
        );
        assert!(
            adi_tools::Tools::with_config(market.config().clone())
                .get("csv-import")
                .expect("get")
                .is_some(),
            "the sibling tool is untouched"
        );
        let ledger = read_ledger(&market, "adi", "uninst-sib-bundle").expect("ledger still exists");
        assert_eq!(ledger.elements.len(), 1);
        assert!(
            bundle_dir_of(&market, "uninst-sib-bundle").exists(),
            "the clone is kept while a sibling remains"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_the_last_element_removes_the_ledger_and_the_clone() {
        let (market, ..) = fixture("uninst-last", "uninst-last-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        unwrap_bundle(install(&market, "adi/uninst-last-bundle", "", false).expect("install"));
        let done = uninstall_element(&market, "adi/uninst-last-bundle/agents/sales-bot").expect("uninstall");
        assert!(done.bundle_removed);
        assert!(read_ledger(&market, "adi", "uninst-last-bundle").is_none());
        assert!(!bundle_dir_of(&market, "uninst-last-bundle").exists());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_an_embedding_backend_in_use_is_refused() {
        let (market, ..) = fixture("uninst-embed", "uninst-embed-bundle", |root| {
            write_embedding(root, "e5");
        });
        let installed = unwrap_bundle(install(&market, "adi/uninst-embed-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Embedding, "e5").id.clone().expect("id");
        let mut settings = adi_embeddings::EmbeddingSettings::open(market.config()).expect("settings");
        settings
            .assignments
            .insert(adi_embeddings::CONSUMER_INDEXER.to_string(), id.clone());
        settings
            .save(&market.config().module(adi_embeddings::EMBEDDINGS_MODULE))
            .expect("save settings");

        let err = uninstall_element(&market, "adi/uninst-embed-bundle/embeddings/e5").expect_err("refused");
        assert!(matches!(err, Error::EmbeddingInUse(_)), "{err}");
        assert!(
            adi_embeddings::EmbeddingBackends::with_config(market.config().clone())
                .get(&id)
                .expect("get")
                .is_some(),
            "untouched by the refusal"
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_a_parked_docker_service_says_the_container_survives() {
        let (market, ..) = fixture("uninst-docker", "uninst-docker-bundle", |root| {
            write_docker_service(root, "db");
        });
        unwrap_bundle(install(&market, "adi/uninst-docker-bundle", "", false).expect("install"));
        let done = uninstall_element(&market, "adi/uninst-docker-bundle/services/db").expect("uninstall");
        assert!(done.note.as_deref().is_some_and(|n| n.contains("docker stop")), "{done:?}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_a_started_service_drops_its_hive_yaml_key() {
        let (market, ..) = fixture("uninst-started-svc", "uninst-started-svc-bundle", |root| {
            write_service(root, "redis");
        });
        unwrap_bundle(install(&market, "adi/uninst-started-svc-bundle", "", false).expect("install"));
        start_service(&market, "adi/uninst-started-svc-bundle/services/redis").expect("start");
        let global_hive = market.config().module("hive").raw_path("hive.yaml");
        let id = read_ledger(&market, "adi", "uninst-started-svc-bundle")
            .expect("ledger")
            .find(Kind::Service, "redis")
            .expect("service")
            .id
            .clone();
        assert!(read_service_block(&global_hive, &id).is_some());

        let done = uninstall_element(&market, "adi/uninst-started-svc-bundle/services/redis").expect("uninstall");
        assert!(done.note.is_none(), "{done:?}");
        assert!(read_service_block(&global_hive, &id).is_none());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_the_whole_bundle_is_refused_there_is_no_such_verb() {
        let (market, ..) = fixture("uninst-whole", "uninst-whole-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        unwrap_bundle(install(&market, "adi/uninst-whole-bundle", "", false).expect("install"));
        let err = uninstall_element(&market, "adi/uninst-whole-bundle").expect_err("refused");
        assert!(matches!(err, Error::BadAddress(_)), "{err}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_something_not_installed_is_refused() {
        let (market, ..) = fixture("uninst-missing", "uninst-missing-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_tool(root, "csv-import");
        });
        unwrap_bundle(install(&market, "adi/uninst-missing-bundle/agents/sales-bot", "", false).expect("install"));
        let err = uninstall_element(&market, "adi/uninst-missing-bundle/tools/csv-import").expect_err("refused");
        assert!(matches!(err, Error::ElementNotInstalled(_)), "{err}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_a_dashboard_element_removes_its_directory() {
        let (market, ..) = fixture("uninst-dash", "uninst-dash-bundle", |root| {
            write_dashboard(root, "crm");
        });
        let installed = unwrap_bundle(install(&market, "adi/uninst-dash-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Dashboard, "crm").id.clone().expect("id");
        uninstall_element(&market, "adi/uninst-dash-bundle/dashboards/crm").expect("uninstall");
        assert!(!market.dashboards_dir().join(&id).exists());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn uninstalling_a_project_scaffold_removes_it() {
        let (market, ..) = fixture("uninst-proj", "uninst-proj-bundle", |root| {
            write_project(root, "Uninst Proj");
        });
        unwrap_bundle(install(&market, "adi/uninst-proj-bundle", "", false).expect("install"));
        let done = uninstall_element(&market, "adi/uninst-proj-bundle/project").expect("uninstall");
        assert!(done.bundle_removed);
        assert!(
            adi_projects::Projects::with_config(market.config().clone())
                .get("uninst-proj-bundle")
                .expect("get")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    // MARK: starting a parked service

    #[test]
    fn starting_a_parked_service_lands_in_the_global_hive_and_drops_the_parked_file() {
        let (market, ..) = fixture("start-global", "start-global-bundle", |root| {
            write_service(root, "redis");
        });
        let installed = unwrap_bundle(install(&market, "adi/start-global-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Service, "redis").id.clone().expect("id");
        let parked = services_dir(&market, "adi", "start-global-bundle").join(format!("{id}.yaml"));
        assert!(parked.is_file());

        let started = start_service(&market, "adi/start-global-bundle/services/redis").expect("start");
        assert_eq!(started.project, None);
        assert!(!parked.exists(), "moved, not copied");
        let global_hive = market.config().module("hive").raw_path("hive.yaml");
        let block = read_service_block(&global_hive, &id).expect("block");
        assert!(block.contains("redis.adi"), "{block}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn starting_a_service_lands_in_the_bundles_own_project_hive_yaml() {
        let (market, ..) = fixture("start-proj", "start-proj-bundle", |root| {
            write_project(root, "Start Proj");
            write_service(root, "redis");
        });
        let installed = unwrap_bundle(install(&market, "adi/start-proj-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Service, "redis").id.clone().expect("id");
        let project_id = installed.project.clone().expect("project");

        let started = start_service(&market, "adi/start-proj-bundle/services/redis").expect("start");
        assert_eq!(started.project.as_deref(), Some(project_id.as_str()));
        let projects = adi_projects::Projects::with_config(market.config().clone());
        let hive_path = projects.hive_path(&project_id).expect("hive path");
        assert!(read_service_block(&hive_path, &id).is_some());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn starting_is_idempotent_when_already_started() {
        let (market, ..) = fixture("start-idem", "start-idem-bundle", |root| {
            write_service(root, "redis");
        });
        unwrap_bundle(install(&market, "adi/start-idem-bundle", "", false).expect("install"));
        start_service(&market, "adi/start-idem-bundle/services/redis").expect("start");
        let again = start_service(&market, "adi/start-idem-bundle/services/redis").expect("start again");
        assert_eq!(again.name, "redis");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn starting_refuses_when_the_target_hive_already_names_that_key() {
        let (market, ..) = fixture("start-collide", "start-collide-bundle", |root| {
            write_service(root, "redis");
        });
        let installed = unwrap_bundle(install(&market, "adi/start-collide-bundle", "", false).expect("install"));
        let id = outcome_of(&installed, Kind::Service, "redis").id.clone().expect("id");
        let global_hive = market.config().module("hive").raw_path("hive.yaml");
        write_service_block(&global_hive, &id, "proxy:\n  host: somebody-elses.adi\n").expect("seed stranger");

        let err = start_service(&market, "adi/start-collide-bundle/services/redis").expect_err("refused");
        assert!(matches!(err, Error::Store(_)), "{err}");
        let block = read_service_block(&global_hive, &id).expect("still there");
        assert!(block.contains("somebody-elses.adi"), "untouched by the refusal");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn starting_an_uninstalled_service_is_refused() {
        let (market, ..) = fixture("start-missing", "start-missing-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        unwrap_bundle(install(&market, "adi/start-missing-bundle", "", false).expect("install"));
        let err = start_service(&market, "adi/start-missing-bundle/services/redis").expect_err("refused");
        assert!(matches!(err, Error::ElementNotInstalled(_)), "{err}");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    // MARK: status — the listing's own "installed as a fraction"

    /// Sync a source called `adi` whose one bundle is `slug`, publishing a two-element preview
    /// (`agents/sales-bot`, `tools/csv-import`) so [`status`]'s `declared` has a real denominator.
    fn synced_with_preview(market: &Marketplace, slug: &str, repo: &str, commit: &str) {
        let manifest = format!(
            r#"{{"name":"t","bundles":[{{"slug":"{slug}","name":"{slug}","repo":"{repo}","commit":"{commit}",
               "elements":[{{"kind":"agent","name":"sales-bot"}},{{"kind":"tool","name":"csv-import"}}]}}]}}"#
        );
        if crate::sources::list(market.config()).expect("sources").is_empty() {
            crate::sources::add(market.config(), "adi", "https://example/marketplace.json")
                .expect("add");
        }
        crate::sync::sync_with(market, |_| Ok(manifest.clone().into_bytes())).expect("sync");
    }

    fn entry_of_adi(market: &Marketplace, slug: &str) -> BundleEntry {
        crate::install::entry_of(market.config(), "adi", slug).expect("cached entry")
    }

    #[test]
    fn status_is_none_before_anything_is_installed() {
        let (market, ..) = fixture("status-none", "status-none-bundle", |root| {
            write_agent(root, "sales-bot", "");
        });
        let entry = entry_of_adi(&market, "status-none-bundle");
        assert_eq!(status(&market, "adi", "status-none-bundle", &entry), None);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn status_lists_every_declared_element_unstalled_when_the_preview_exists_but_nothing_is() {
        // A manifest that previews two elements, and an install call nobody has made yet — the
        // only list there is to show before anything is cloned (decision #2), and it should read
        // as "nothing installed yet," not as "this bundle carries nothing."
        let (market, repo, commit) = fixture("status-preview-only", "preview-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_tool(root, "csv-import");
        });
        synced_with_preview(&market, "preview-bundle", &repo, &commit);
        let entry = entry_of_adi(&market, "preview-bundle");
        let s = status(&market, "adi", "preview-bundle", &entry).expect("a preview is something to say");
        assert_eq!(s.declared, 2);
        assert!(s.installed.is_empty(), "nothing has been installed");
        assert_eq!(s.elements.len(), 2, "{:?}", s.elements);
        assert!(s.elements.iter().all(|row| row.id.is_none()), "{:?}", s.elements);
        assert_eq!(s.elements[0].kind, Kind::Agent, "agents sort ahead of tools (Kind::ALL)");
        assert_eq!(s.elements[0].name, "sales-bot");
        assert_eq!(s.elements[1].kind, Kind::Tool);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn status_reports_the_installed_fraction_and_whether_it_is_outdated() {
        let (market, repo, commit) = fixture("status-fraction", "status-bundle", |root| {
            write_agent(root, "sales-bot", "");
            write_tool(root, "csv-import");
        });
        synced_with_preview(&market, "status-bundle", &repo, &commit);

        // Only one of the two declared elements is installed.
        unwrap_bundle(install(&market, "adi/status-bundle/agents/sales-bot", "", false).expect("install"));
        let entry = entry_of_adi(&market, "status-bundle");
        let s = status(&market, "adi", "status-bundle", &entry).expect("installed");
        assert_eq!(s.declared, 2, "the manifest's own preview");
        assert_eq!(s.installed.len(), 1, "only the agent landed");
        assert_eq!(s.installed[0].kind, Kind::Agent);
        assert!(!s.outdated, "installed at the pin the manifest names now");

        // The merged rows carry both: the agent with the id it landed as, and the tool the
        // manifest still previews but nothing has installed.
        assert_eq!(s.elements.len(), 2, "{:?}", s.elements);
        let agent_row = s.elements.iter().find(|r| r.kind == Kind::Agent).expect("agent row");
        assert_eq!(agent_row.id.as_deref(), Some("sales-bot"));
        let tool_row = s.elements.iter().find(|r| r.kind == Kind::Tool).expect("tool row");
        assert_eq!(tool_row.id, None, "declared, not yet installed");

        // The publisher cuts a new release; the ledger's own clone is now behind it.
        advance(&market, "status-bundle", &repo, |root| {
            write_agent(root, "sales-bot", "label = \"v2\"\n");
        });
        let entry = entry_of_adi(&market, "status-bundle");
        let s = status(&market, "adi", "status-bundle", &entry).expect("still installed");
        assert!(s.outdated, "the manifest moved on and the ledger has not");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    /// Sync a source called `adi` whose one bundle publishes a preview naming its project
    /// scaffold something other than the bundle's own slug — exactly what a real publisher does
    /// (`project`/`config`, say) and exactly the shape that broke the merge in [`status`]: the
    /// scaffold always lands under the bundle's slug (`install`'s own `addr.slug.clone()`), never
    /// under the preview's own name, so matching the two by name would show the one scaffold
    /// twice.
    fn synced_with_project_preview(market: &Marketplace, slug: &str, repo: &str, commit: &str) {
        let manifest = format!(
            r#"{{"name":"t","bundles":[{{"slug":"{slug}","name":"{slug}","repo":"{repo}","commit":"{commit}",
               "elements":[{{"kind":"project","name":"config","description":"The scaffold."}}]}}]}}"#
        );
        if crate::sources::list(market.config()).expect("sources").is_empty() {
            crate::sources::add(market.config(), "adi", "https://example/marketplace.json")
                .expect("add");
        }
        crate::sync::sync_with(market, |_| Ok(manifest.clone().into_bytes())).expect("sync");
    }

    #[test]
    fn status_never_lists_the_project_scaffold_twice_under_two_different_names() {
        let (market, repo, commit) = fixture("status-project", "status-project-bundle", |root| {
            write_project(root, "Status Project Bundle");
        });
        synced_with_project_preview(&market, "status-project-bundle", &repo, &commit);

        // Before install: one row, predicting the slug it will actually land under — not the
        // preview's own "config", which an address never names anyway.
        let entry = entry_of_adi(&market, "status-project-bundle");
        let s = status(&market, "adi", "status-project-bundle", &entry).expect("a preview exists");
        assert_eq!(s.elements.len(), 1, "{:?}", s.elements);
        assert_eq!(s.elements[0].kind, Kind::Project);
        assert_eq!(s.elements[0].name, "status-project-bundle");
        assert_eq!(s.elements[0].id, None);
        assert_eq!(s.elements[0].description.as_deref(), Some("The scaffold."));

        // Installed: still one row, now carrying the id it landed as and the preview's own
        // description — not two, one for "config" and one for whatever it actually landed as.
        unwrap_bundle(install(&market, "adi/status-project-bundle", "", false).expect("install"));
        let s = status(&market, "adi", "status-project-bundle", &entry).expect("installed");
        assert_eq!(s.elements.len(), 1, "{:?}", s.elements);
        assert_eq!(s.elements[0].kind, Kind::Project);
        assert_eq!(s.elements[0].id.as_deref(), Some("status-project-bundle"));
        assert_eq!(s.elements[0].description.as_deref(), Some("The scaffold."));
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn status_recomputes_missing_secrets_live_rather_than_freezing_them_at_install() {
        let (market, ..) = fixture("status-secret", "status-secret-bundle", |root| {
            write_agent(root, "sales-bot", "[[secrets]]\nname = \"OPENAI_API_KEY\"\n");
        });
        unwrap_bundle(install(&market, "adi/status-secret-bundle", "", false).expect("install"));
        let entry = entry_of_adi(&market, "status-secret-bundle");
        let s = status(&market, "adi", "status-secret-bundle", &entry).expect("installed");
        assert_eq!(s.missing_secrets, vec!["OPENAI_API_KEY".to_string()]);

        // Setting the secret afterwards is reflected without reinstalling anything.
        adi_secrets::Secrets::with_config(market.config().clone())
            .set(None, "OPENAI_API_KEY", "sk-test", None)
            .expect("set");
        let s = status(&market, "adi", "status-secret-bundle", &entry).expect("still installed");
        assert!(s.missing_secrets.is_empty(), "{:?}", s.missing_secrets);
        let _ = std::fs::remove_dir_all(market.config().root());
    }
}
