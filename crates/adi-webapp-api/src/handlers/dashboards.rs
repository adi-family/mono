//! `GET /api/dashboards` — the dashboards under `~/.adi/mono/dashboards/<id>/`.
//!
//! A dashboard is a bun-served frontend/backend pair whose UI is authored as loose `.ts` files:
//! `frontend/modules/*.ts` are the panels, `backend/routes/*.ts` the endpoints. Only the two
//! `index.ts` entry points are fixed, so listing those two directories is what tells a reader
//! what a given dashboard actually does.
//!
//! Neither port is declared in the dashboard's `hive.yaml`: adi-hive leases one per service
//! from the ports manager, keyed `<id>/frontend` and `<id>/backend`. We resolve them from that
//! same registry, which is also why a dashboard can report ports before it is running.
//!
//! **One dashboard is one origin** (`docs/fleet.md` §4). Both services declare the *same*
//! `proxy.host`; the frontend owns `/` and the backend claims `/api` through hive path routing.
//! That is what lets the page use relative URLs only and never learn its own address — the
//! precondition for the same dashboard working at `<host>.adi`, at `<host>.<node>.n.adi` over
//! the mesh (where `127.0.0.1` would be the *viewer's* machine), and behind a real domain later.
//! Dashboards written before that rule are brought up to it by [`migrate`] on the next read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use adi_config::Config;
use adi_ports_manager::Ports;
use adi_projects::Projects;
use base64::Engine as _;
use serde::Deserialize;

use crate::types::{
    BundleFile, Dashboard, DashboardBundle, DashboardRef, DashboardsState, NewDashboard,
    SetDashboardProject, UsedPort,
};

use super::response::{Response, error, ok_json};
use super::services::is_listening;

/// The metadata file each dashboard directory carries.
#[derive(Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// The project this dashboard is filed under (its id), or `None` when unfiled.
    #[serde(default)]
    project: Option<String>,
    /// When the dashboard was archived (Unix seconds), or `None` while it is live.
    #[serde(default)]
    archived_at: Option<u64>,
    /// The node this dashboard was moved to, when it was ([`complete_move`]). Written beside
    /// `archived_at` rather than instead of it: the local remains are archived like any other
    /// archived dashboard, and this only says *why*.
    #[serde(default)]
    moved_to: Option<String>,
}

/// The scaffold a new dashboard starts from — the two fixed entry points plus one worked
/// example of each extension point, embedded so the binary can create a dashboard anywhere.
const FRONTEND_INDEX_TS: &str = include_str!("../../templates/dashboard/frontend/index.ts");
const FRONTEND_INDEX_HTML: &str = include_str!("../../templates/dashboard/frontend/index.html");
const FRONTEND_MODULE_STATUS: &str =
    include_str!("../../templates/dashboard/frontend/modules/status.ts");
const BACKEND_INDEX_TS: &str = include_str!("../../templates/dashboard/backend/index.ts");
const BACKEND_ROUTE_STATUS: &str =
    include_str!("../../templates/dashboard/backend/routes/status.ts");
const README: &str = include_str!("../../templates/dashboard/README.md");

/// `POST /api/dashboards/create` — scaffold a new dashboard and let the supervisor pick it up.
///
/// Writing the files is the whole job: the per-user dashboards hive re-reads its imports every
/// few seconds, so it leases the ports and starts both bun servers on its own. The response
/// therefore carries no ports yet — poll `GET /api/dashboards` (or let the page refresh) and
/// they appear once the supervisor has reconciled.
#[must_use]
pub fn create_dashboard(cfg: &Config, ports: &Ports, body: &[u8]) -> Response {
    let req: NewDashboard = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let name = req.name.trim();
    if name.is_empty() {
        return error(400, "name must not be empty");
    }

    let id = uuid::Uuid::new_v4().to_string();
    let dir = cfg.module("dashboards").dir().join(&id);
    let project = req.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    if let Err(e) = scaffold(
        &dir,
        name,
        req.description.as_deref().unwrap_or("").trim(),
        project,
    ) {
        // A half-written directory would be picked up by the supervisor as a broken service,
        // so clear it rather than leave the tree in a state nobody asked for.
        let _ = std::fs::remove_dir_all(&dir);
        return error(500, &format!("could not create dashboard: {e}"));
    }

    ok_json(&read_dashboard(&dir, ports, &[]))
}

/// `POST /api/dashboards/archive` — soft-remove a dashboard, then report the fresh listing.
///
/// Archiving records `archived_at` in the manifest and parks the hive file so the supervisor's
/// import glob no longer matches it — both bun servers stop within a few seconds — without
/// deleting anything. The row moves to the page's Archived disclosure, from where Restore undoes it.
#[must_use]
pub fn archive_dashboard(cfg: &Config, ports: &Ports, live: &[UsedPort], body: &[u8]) -> Response {
    set_archived(cfg, ports, live, body, true)
}

/// `POST /api/dashboards/unarchive` — restore an archived dashboard, then report the fresh
/// listing. Moves the hive file back into the supervisor's glob (so both servers restart on the
/// same leased ports) and clears `archived_at`.
#[must_use]
pub fn unarchive_dashboard(
    cfg: &Config,
    ports: &Ports,
    live: &[UsedPort],
    body: &[u8],
) -> Response {
    set_archived(cfg, ports, live, body, false)
}

/// `POST /api/dashboards/delete` — permanently delete an archived dashboard's directory (all its
/// files), then report the fresh listing. Refused with a 409 unless the dashboard is archived
/// first, so a live, supervised dashboard is never pulled out from under its running bun servers.
/// Irreversible — the UI gates it behind a confirm.
#[must_use]
pub fn delete_dashboard(cfg: &Config, ports: &Ports, live: &[UsedPort], body: &[u8]) -> Response {
    let Some(id) = parse_dashboard_ref(body) else {
        return error(400, "expected JSON body { \"id\": \"…\" }");
    };
    let Some(dir) = dashboard_dir(cfg, &id) else {
        return error(404, &format!("no such dashboard: {id}"));
    };
    if read_manifest(&dir).archived_at.is_none() {
        return error(409, "archive the dashboard before deleting it");
    }
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        return error(500, &format!("could not delete dashboard: {e}"));
    }
    dashboards(cfg, ports, live)
}

/// Shared body of archive/unarchive: validate the id, flip the manifest's `archived_at`, move the
/// hive file into or out of the supervisor's glob, then answer with the fresh full listing.
fn set_archived(
    cfg: &Config,
    ports: &Ports,
    live: &[UsedPort],
    body: &[u8],
    archived: bool,
) -> Response {
    let Some(id) = parse_dashboard_ref(body) else {
        return error(400, "expected JSON body { \"id\": \"…\" }");
    };
    let Some(dir) = dashboard_dir(cfg, &id) else {
        return error(404, &format!("no such dashboard: {id}"));
    };

    let mut manifest = read_manifest(&dir);
    manifest.archived_at = archived.then(now_secs);
    // Restoring makes this machine the one that runs it again, so the "moved to <node>" note has
    // to go with it — leaving it would label a live dashboard as living somewhere else.
    if !archived {
        manifest.moved_to = None;
    }
    if let Err(e) = write_manifest(&dir, &manifest) {
        return error(500, &format!("could not update dashboard manifest: {e}"));
    }

    // Park the hive file aside (archive) or move it back (restore). Renaming it out of the
    // `**/hive.yaml` glob is what actually stops the supervised servers.
    let supervised = dir.join(".adi").join(HIVE_LIVE);
    let parked = dir.join(".adi").join(HIVE_ARCHIVED);
    let (from, to) = if archived {
        (&supervised, &parked)
    } else {
        (&parked, &supervised)
    };
    // Best-effort: a dashboard with no hive file (or already in the target state) has nothing to
    // move, which is not an error — the manifest flag above is the source of truth.
    if from.exists()
        && let Err(e) = std::fs::rename(from, to)
    {
        return error(500, &format!("could not move dashboard hive file: {e}"));
    }

    dashboards(cfg, ports, live)
}

/// `POST /api/dashboards/project` — file a dashboard under a project (or unfile it with an empty
/// `project`), then report the fresh listing. Purely a manifest edit: the dashboard keeps running
/// on its own port, so nothing is restarted.
#[must_use]
pub fn set_dashboard_project(
    cfg: &Config,
    ports: &Ports,
    live: &[UsedPort],
    body: &[u8],
) -> Response {
    let req: SetDashboardProject = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return error(400, &format!("invalid request body: {e}")),
    };
    let Some(dir) = dashboard_dir(cfg, &req.id) else {
        return error(404, &format!("no such dashboard: {}", req.id));
    };
    let mut manifest = read_manifest(&dir);
    manifest.project = req
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    if let Err(e) = write_manifest(&dir, &manifest) {
        return error(500, &format!("could not update dashboard manifest: {e}"));
    }
    dashboards(cfg, ports, live)
}

/// Resolve a client-supplied dashboard id to its directory, refusing anything that isn't a single
/// path segment naming an existing dashboard — so the id can never climb out of the dashboards
/// root.
fn dashboard_dir(cfg: &Config, id: &str) -> Option<PathBuf> {
    let id = id.trim();
    if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\\') {
        return None;
    }
    let dir = cfg.module("dashboards").dir().join(id);
    dir.is_dir().then_some(dir)
}

/// Parse a [`DashboardRef`] body into its trimmed, non-empty id.
fn parse_dashboard_ref(body: &[u8]) -> Option<String> {
    let req: DashboardRef = serde_json::from_slice(body).ok()?;
    let id = req.id.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// The current Unix time in whole seconds (0 before the epoch, which never happens in practice).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Write the full scaffold into `dir`. Any error leaves the caller to clean up.
fn scaffold(
    dir: &Path,
    name: &str,
    description: &str,
    project: Option<&str>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join("frontend").join("modules"))?;
    std::fs::create_dir_all(dir.join("backend").join("routes"))?;
    std::fs::create_dir_all(dir.join(".adi"))?;

    std::fs::write(dir.join("frontend").join("index.ts"), FRONTEND_INDEX_TS)?;
    std::fs::write(dir.join("frontend").join("index.html"), FRONTEND_INDEX_HTML)?;
    std::fs::write(
        dir.join("frontend").join("modules").join("status.ts"),
        FRONTEND_MODULE_STATUS,
    )?;
    std::fs::write(dir.join("backend").join("index.ts"), BACKEND_INDEX_TS)?;
    std::fs::write(
        dir.join("backend").join("routes").join("status.ts"),
        BACKEND_ROUTE_STATUS,
    )?;

    let id = dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    std::fs::write(
        dir.join("README.md"),
        README.replace("{{NAME}}", name).replace("{{ID}}", &id),
    )?;

    write_manifest(
        dir,
        &Manifest {
            name: Some(name.to_string()),
            description: Some(description.to_string()),
            project: project.map(str::to_string),
            archived_at: None,
            moved_to: None,
        },
    )?;
    let host = dashboard_host(dir, name);
    std::fs::write(dir.join(".adi").join(HIVE_LIVE), hive_yaml(dir, &host))?;
    Ok(())
}

/// The dashboard's hive file, as the supervisor's `$ADI_DASHBOARDS_DIR/**/hive.yaml` glob names
/// it. Archiving parks it aside under [`HIVE_ARCHIVED`] (which the glob no longer matches), so
/// the supervisor drops both bun services within a few seconds; restoring moves it back.
const HIVE_LIVE: &str = "hive.yaml";
/// The parked name an archived dashboard's hive file takes — deliberately not `hive.yaml`, so the
/// supervisor's glob skips it.
const HIVE_ARCHIVED: &str = "hive.yaml.archived";

/// Read a dashboard directory's `config.toml` manifest, degrading a missing or malformed file to
/// the default (all fields absent) rather than failing.
fn read_manifest(dir: &Path) -> Manifest {
    std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|raw| toml::from_str::<Manifest>(&raw).ok())
        .unwrap_or_default()
}

/// Write a dashboard's `config.toml`, emitting only the fields that are present so a rewrite never
/// invents a blank `name`/`description` the manifest didn't already carry.
fn write_manifest(dir: &Path, manifest: &Manifest) -> std::io::Result<()> {
    let mut out = String::new();
    if let Some(name) = &manifest.name {
        out.push_str(&format!("name = {}\n", toml_string(name)));
    }
    if let Some(description) = &manifest.description {
        out.push_str(&format!("description = {}\n", toml_string(description)));
    }
    if let Some(project) = &manifest.project {
        out.push_str(&format!("project = {}\n", toml_string(project)));
    }
    if let Some(ts) = manifest.archived_at {
        out.push_str(&format!("archived_at = {ts}\n"));
    }
    if let Some(node) = &manifest.moved_to {
        out.push_str(&format!("moved_to = {}\n", toml_string(node)));
    }
    std::fs::write(dir.join("config.toml"), out)
}

/// The dashboard's hive services: **one host, two services**. `{{HOST}}` is the hostname both
/// share and `{{DIR}}` the dashboard directory; nothing else is generated, and in particular no
/// port ever is — adi-hive leases those.
///
/// Kept as a template rather than a `format!` chain so the emitted YAML reads here exactly as it
/// lands on disk, comments and all.
const HIVE_TEMPLATE: &str = r#"# Dashboard hive services — run by the per-user supervisor (~/.adi/mono/dashboards/hive.yaml).
#
# One dashboard is one origin. Both services declare the same `proxy.host`: the frontend owns
# `/`, the backend claims `/api`. The page therefore only ever uses relative URLs and never
# learns its own address, which is what lets this dashboard work unchanged at `{{HOST}}`, at
# `<label>.<node>.n.adi` over the mesh, and behind a real domain later — for every viewer, with
# no substitution. Do not give the backend a host of its own: an absolute backend URL in the page
# would point at whatever machine the *browser* is on.
#
# The front door imports dashboards (stripping their runners, since it only routes) and picks
# both entries up; the per-user supervisor is what actually runs them.
#
# No port is declared: adi-hive leases a stable one per service from the ports manager (keyed
# `<dashboard-id>/frontend` and `<dashboard-id>/backend`) and injects it as $PORT. The leases are
# idempotent, so the front door resolves the same port the supervisor runs on.

version: "1"

services:
  frontend:
    restart: always
    proxy:
      host: {{HOST}}
    runner:
      type: script
      script:
        run: bun run frontend/index.ts
        working_dir: {{DIR}}

  backend:
    restart: always
    proxy:
      host: {{HOST}}
      path: /api
    runner:
      type: script
      script:
        run: bun run backend/index.ts
        working_dir: {{DIR}}
"#;

/// Render [`HIVE_TEMPLATE`] for one dashboard directory and hostname.
fn hive_yaml(dir: &Path, host: &str) -> String {
    HIVE_TEMPLATE
        .replace("{{HOST}}", host)
        .replace("{{DIR}}", &dir.display().to_string())
}

/// The zone every local service answers under, so a label becomes `<label>.adi`.
const HOST_ZONE: &str = "adi";

/// The path prefix the backend claims on the dashboard's host. The page's whole API base.
const API_PATH: &str = "/api";

/// The longest a single DNS label may be.
const MAX_LABEL: usize = 63;

/// Labels a dashboard may never take, because something else already answers there:
/// `n` is the reserved mesh zone (`docs/fleet.md` §1, and adi-hive refuses to route `n.adi`),
/// `app` is the control panel, and the rest would shadow infrastructure or read as one.
const RESERVED_LABELS: &[&str] = &["adi", "api", "app", "dns", "hive", "localhost", "n", "www"];

/// The label used when neither the name nor the id yields a usable one — unreachable in practice
/// (an id is a UUID, which is already a valid label), but a host must always exist.
const FALLBACK_LABEL: &str = "dashboard";

/// The hostname both of a dashboard's services share: `<label>.adi`.
///
/// Deterministic, and derived from what a human already typed: a slug of the dashboard's name,
/// falling back to its id when the name has nothing DNS-usable in it (all-unicode, punctuation
/// only), is reserved, or is already claimed by another dashboard. The id is a UUID, so that
/// fallback is always free — a collision costs you a pretty hostname, never a working one.
fn dashboard_host(dir: &Path, name: &str) -> String {
    let id = dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    format!("{}.{HOST_ZONE}", host_label(dir, &id, name))
}

/// The label part of [`dashboard_host`]. Split out so the fallback chain is testable on its own.
fn host_label(dir: &Path, id: &str, name: &str) -> String {
    let taken = claimed_labels(dir);
    let free = |label: &String| !is_reserved(label) && !taken.contains(label);

    slugify(name)
        .filter(free)
        .or_else(|| slugify(id).filter(|l| !is_reserved(l)))
        .unwrap_or_else(|| FALLBACK_LABEL.to_string())
}

/// Whether `label` is one of the names a dashboard must not take. Compared case-insensitively
/// even though [`slugify`] already lowercases, so a hand-edited hive file is judged the same way.
fn is_reserved(label: &str) -> bool {
    RESERVED_LABELS.contains(&label.to_ascii_lowercase().as_str())
}

/// Reduce free text to a single DNS label: ASCII-lowercased, every other character a separator,
/// runs of separators collapsed, trimmed, capped at [`MAX_LABEL`]. `None` when nothing usable is
/// left — a name written entirely in a non-Latin script is the common case, and inventing a
/// transliteration for it would be a worse hostname than the id.
fn slugify(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(MAX_LABEL);
    let label = out.trim_matches('-').to_string();
    (!label.is_empty()).then_some(label)
}

/// Every host label already claimed by a dashboard *other* than the one in `dir`.
///
/// Read from the siblings' hive files rather than re-derived from their names, so a host that was
/// hand-picked (or derived under an older rule) still counts as taken — two dashboards answering
/// on one hostname is a routing coin-flip, and the point of the check is that it never happens.
fn claimed_labels(dir: &Path) -> BTreeSet<String> {
    let Some(root) = dir.parent() else {
        return BTreeSet::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p != dir)
        .filter_map(|p| declared_host(&p))
        .map(|host| label_of(&host))
        .collect()
}

/// The first label of a hostname, lowercased — `nosh.adi` → `nosh`.
fn label_of(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The proxy-relevant subset of a dashboard's hive file — enough to tell whether it already
/// declares one origin, and which host it declares. Unknown fields (runners, restart policy) are
/// ignored: this parse decides *whether* to rewrite, never what to keep.
#[derive(Deserialize)]
struct HiveFile {
    #[serde(default)]
    services: BTreeMap<String, HiveService>,
}

#[derive(Deserialize)]
struct HiveService {
    #[serde(default)]
    proxy: Option<HiveProxy>,
}

#[derive(Deserialize)]
struct HiveProxy {
    host: String,
    #[serde(default)]
    path: Option<String>,
}

/// The dashboard's declared hostname, preferring the frontend's (it owns the host's root) and
/// falling back to the backend's. `None` when there is no hive file, it does not parse, or no
/// service declares a `proxy.host` — all three meaning "nothing has been claimed yet".
fn declared_host(dir: &Path) -> Option<String> {
    let parsed = parse_hive(dir)?.1;
    let host = |svc: &str| {
        parsed
            .services
            .get(svc)
            .and_then(|s| s.proxy.as_ref())
            .map(|p| p.host.trim().to_string())
            .filter(|h| !h.is_empty())
    };
    host("frontend").or_else(|| host("backend"))
}

/// Read and parse whichever of the two hive file names is on disk, returning that path with it.
/// The live name wins; an archived dashboard keeps its parked file, and migrating that one too is
/// what stops a restore from bringing back the old shape.
fn parse_hive(dir: &Path) -> Option<(PathBuf, HiveFile)> {
    let path = [HIVE_LIVE, HIVE_ARCHIVED]
        .iter()
        .map(|f| dir.join(".adi").join(f))
        .find(|p| p.is_file())?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let parsed = serde_yaml_ng::from_str::<HiveFile>(&raw).ok()?;
    Some((path, parsed))
}

// MARK: migration — dashboards written before the one-origin rule

/// What a pre-one-origin frontend entry point still carries: the injected `backendPort`, read out
/// of the ports registry and handed to the browser so it could dial `127.0.0.1:<port>` itself.
/// Both `frontend/index.ts` and the shell it renders spell it, and neither template does now.
const LEGACY_BACKEND_PORT: &str = "backendPort";

/// What a pre-one-origin backend entry point still carries: wildcard CORS, which only ever
/// existed because the page called it from a different origin. Under one origin it is not merely
/// unnecessary — it lets any page you visit read this dashboard's API.
const LEGACY_CORS: &str = "access-control-allow-origin";

/// Bring a dashboard written before the one-origin rule (`docs/fleet.md` §4) up to it, in place,
/// the next time it is read or listed. There is no separate migration command: a dashboard is a
/// directory, and the listing is the only thing guaranteed to visit every one of them.
///
/// Idempotent by construction — every step tests what is on disk and writes only when the old
/// shape is still there, so the panel's few-second poll writes nothing once a dashboard is
/// current, and the supervisor sees no spurious config change.
///
/// It rewrites **generated** files only: the hive file and the three fixed entry points. The
/// panels and routes under `frontend/modules/` and `backend/routes/` are what a user or an agent
/// authored, and are never read here, let alone written.
fn migrate(dir: &Path, name: &str) {
    migrate_hive(dir, name);
    let frontend = dir.join("frontend");
    migrate_entry_point(
        &frontend.join("index.ts"),
        LEGACY_BACKEND_PORT,
        FRONTEND_INDEX_TS,
    );
    migrate_entry_point(
        &frontend.join("index.html"),
        LEGACY_BACKEND_PORT,
        FRONTEND_INDEX_HTML,
    );
    migrate_entry_point(
        &dir.join("backend").join("index.ts"),
        LEGACY_CORS,
        BACKEND_INDEX_TS,
    );
}

/// Rewrite one fixed entry point with the current template, but only while it still spells
/// `marker` — the one string the old file has and the new one does not. That test is what makes
/// this both idempotent (the marker is gone after the first pass) and safe to run on every read.
fn migrate_entry_point(path: &Path, marker: &str, template: &str) {
    let Ok(current) = std::fs::read_to_string(path) else {
        return;
    };
    if !current.contains(marker) {
        return;
    }
    let _ = std::fs::write(path, template);
}

/// Rewrite the dashboard's hive file to the one-origin form, keeping the hostname it already
/// declares. A hand-picked host is a link somebody has bookmarked, so migration never re-derives
/// one that exists; a dashboard that declares none gets [`dashboard_host`].
///
/// Anything that is not recognisably this scaffold's own file — unparseable, or carrying services
/// beyond the `frontend`/`backend` pair — is left exactly as it is. A rewrite is a full rewrite,
/// and clobbering a hive file we do not understand would cost more than the stale shape does.
fn migrate_hive(dir: &Path, name: &str) {
    let Some((path, parsed)) = parse_hive(dir) else {
        return;
    };
    let services: Vec<&str> = parsed.services.keys().map(String::as_str).collect();
    if services != ["backend", "frontend"] {
        return;
    }
    if is_one_origin(&parsed) {
        return;
    }

    let host = declared_host(dir).unwrap_or_else(|| dashboard_host(dir, name));
    let _ = std::fs::write(path, hive_yaml(dir, &host));
}

/// Whether a parsed hive file already declares one origin: both services on the same host, the
/// frontend as that host's fallback route, the backend claiming [`API_PATH`]. Paths are compared
/// after the same normalisation adi-hive applies, so `/api/` and `api` count as current.
fn is_one_origin(parsed: &HiveFile) -> bool {
    let proxy = |svc: &str| parsed.services.get(svc).and_then(|s| s.proxy.as_ref());
    let (Some(frontend), Some(backend)) = (proxy("frontend"), proxy("backend")) else {
        return false;
    };
    let same_host = !frontend.host.trim().is_empty()
        && frontend.host.trim().eq_ignore_ascii_case(backend.host.trim());
    same_host
        && path_claim(frontend.path.as_deref()).is_none()
        && path_claim(backend.path.as_deref()).as_deref() == Some(API_PATH)
}

/// Normalise a `proxy.path` the way adi-hive's router does: `None` (the host's fallback) or a
/// `/`-rooted prefix with no trailing slash. `/` is the fallback, so it normalises to `None`.
fn path_claim(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    })
}

// MARK: moving a dashboard to another machine

/// The most a bundle may carry, in raw bytes. Generous for what a dashboard is — a handful of
/// `.ts` files and whatever assets go with them — and small enough that the whole thing fits in
/// one JSON body on both ends after base64 has added its third.
///
/// A cap rather than a stream because the alternative is worse: a transfer that half-arrives
/// leaves a dashboard on the node with some of its modules, which looks like it worked.
const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024;

/// The most files a bundle may carry. A dashboard someone pointed at a data directory is the
/// case this exists for — it fails with a sentence rather than a five-minute walk.
const MAX_BUNDLE_FILES: usize = 2000;

/// Directory names never packed, wherever they appear in the tree.
///
/// `.adi` because the hive file inside it is rebuilt on the far side (its `working_dir` is an
/// absolute local path, and its host may be taken over there); the other two because they are
/// caches of things already in the bundle, and shipping them is how a 20 KB dashboard becomes a
/// 200 MB one.
const NEVER_BUNDLED_DIRS: &[&str] = &[".adi", "node_modules", ".git"];

/// Files never packed from the dashboard's root: the manifest travels as the bundle's own fields,
/// so shipping it too would be two sources of truth for one name.
const NEVER_BUNDLED_ROOT_FILES: &[&str] = &["config.toml"];

/// What lives through an import that overwrites an existing dashboard.
///
/// `.adi` holds the hive file the receiving machine wrote for *its* paths — rewritten right
/// after, but never through a window in which the supervisor could read a missing one. Anything
/// installed under `node_modules` is a cache the bundle deliberately did not carry, and deleting
/// it would make every re-transfer an install.
const KEPT_ON_IMPORT: &[&str] = &[".adi", "node_modules"];

/// Pack a dashboard's authored files into a [`DashboardBundle`] ready to POST at another machine.
///
/// Everything a person or an agent wrote travels; everything a machine generated does not (see
/// [`NEVER_BUNDLED_DIRS`]). Symlinks are skipped rather than followed — a link pointing out of the
/// dashboard would otherwise quietly put whatever it names on the wire.
///
/// # Errors
/// The [`Response`] to answer with: 404 for an unknown id, 413 when the directory is past
/// [`MAX_BUNDLE_BYTES`] / [`MAX_BUNDLE_FILES`], 500 on a read failure.
pub fn export_bundle(cfg: &Config, id: &str) -> Result<DashboardBundle, Response> {
    let Some(dir) = dashboard_dir(cfg, id) else {
        return Err(error(404, &format!("no such dashboard: {id}")));
    };
    let manifest = read_manifest(&dir);
    let name = manifest.name.clone().unwrap_or_else(|| id.to_string());

    let mut files = Vec::new();
    let mut total = 0_u64;
    collect_files(&dir, &mut PathBuf::new(), &mut files, &mut total)?;

    Ok(DashboardBundle {
        id: id.to_string(),
        name,
        description: manifest.description,
        project: manifest.project,
        host: declared_host(&dir),
        files,
    })
}

/// Walk one directory of a dashboard, appending its files to `files`. `rel` is the path so far,
/// relative to the dashboard root, which is what the bundle records.
///
/// Recursive rather than iterative because the depth is a dashboard's own source tree; the two
/// caps are what bound the work, not the shape of the walk.
fn collect_files(
    dir: &Path,
    rel: &mut PathBuf,
    files: &mut Vec<BundleFile>,
    total: &mut u64,
) -> Result<(), Response> {
    let here = dir.join(&*rel);
    let entries = match std::fs::read_dir(&here) {
        Ok(entries) => entries,
        Err(e) => return Err(error(500, &format!("reading {}: {e}", here.display()))),
    };
    // Sorted, so a bundle of an unchanged dashboard is byte-identical between runs and a diff of
    // two transfers is about the dashboard rather than about directory order.
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();

    for name in names {
        if NEVER_BUNDLED_DIRS.contains(&name.as_str())
            || (rel.as_os_str().is_empty() && NEVER_BUNDLED_ROOT_FILES.contains(&name.as_str()))
        {
            continue;
        }
        let path = here.join(&name);
        // Not `metadata`: that follows the link, and a link out of the dashboard would then be
        // read and shipped as though it lived here.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        rel.push(&name);
        let walked = if meta.is_dir() {
            collect_files(dir, rel, files, total)
        } else {
            pack_file(&path, rel, meta.len(), files, total)
        };
        rel.pop();
        walked?;
    }
    Ok(())
}

/// Add one file to the bundle, refusing once either cap is past.
fn pack_file(
    path: &Path,
    rel: &Path,
    size: u64,
    files: &mut Vec<BundleFile>,
    total: &mut u64,
) -> Result<(), Response> {
    *total += size;
    if *total > MAX_BUNDLE_BYTES || files.len() >= MAX_BUNDLE_FILES {
        return Err(error(
            413,
            &format!(
                "this dashboard is too large to transfer ({} files, {} bytes so far; the limits \
                 are {MAX_BUNDLE_FILES} files and {MAX_BUNDLE_BYTES} bytes) — move the bulk of it \
                 out of the dashboard directory, or copy it across by hand",
                files.len() + 1,
                *total,
            ),
        ));
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => return Err(error(500, &format!("reading {}: {e}", path.display()))),
    };
    files.push(BundleFile {
        path: slash_path(rel),
        contents: base64::engine::general_purpose::STANDARD.encode(bytes),
    });
    Ok(())
}

/// A relative path as the bundle spells it: `/`-separated on every platform, so a dashboard
/// packed on Windows unpacks on Linux and the reverse.
fn slash_path(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(segment) => Some(segment.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `POST /api/dashboards/import` — receive a dashboard packed by another machine and make it
/// this machine's own.
///
/// Nothing is started from here: writing `<id>/.adi/hive.yaml` is the whole job, because the
/// per-user supervisor re-reads its import glob every few seconds and leases the ports itself —
/// the same contract [`create_dashboard`] relies on.
///
/// **The same id imported twice updates, it does not duplicate.** That is what makes "transfer"
/// double as "redeploy": edit the dashboard here, send it again, and the copy over there becomes
/// what you have — leased ports, hostname and all — instead of a second row beside it. A mirror
/// and not a merge: files the bundle no longer carries are removed, or a module you deleted would
/// go on being served.
#[must_use]
pub fn import_dashboard(
    projects: &Projects,
    ports: &Ports,
    live: &[UsedPort],
    body: &[u8],
) -> Response {
    let bundle: DashboardBundle = match serde_json::from_slice(body) {
        Ok(bundle) => bundle,
        Err(e) => return error(400, &format!("invalid dashboard bundle: {e}")),
    };
    let name = bundle.name.trim();
    if name.is_empty() {
        return error(400, "the bundle names no dashboard");
    }
    let Some(id) = valid_id(&bundle.id) else {
        return error(400, "the bundle carries no usable dashboard id");
    };
    if bundle.files.len() > MAX_BUNDLE_FILES {
        return error(413, "the bundle carries too many files");
    }
    // An import is a mirror, so an empty one would *empty* a dashboard already running here. A
    // dashboard with no files is not a thing anybody means to send.
    if bundle.files.is_empty() {
        return error(400, "the bundle carries no files");
    }

    let cfg = projects.config();
    let dir = cfg.module("dashboards").dir().join(&id);
    // Decode and path-check *everything* before a single byte is written: a bundle rejected
    // halfway would leave a live dashboard holding a mix of two versions.
    let decoded = match decode_bundle(&dir, &bundle.files) {
        Ok(decoded) => decoded,
        Err(response) => return response,
    };

    if let Err(e) = write_import(&dir, &decoded) {
        return error(500, &format!("writing the dashboard: {e}"));
    }

    // A project id means nothing on this machine unless a project by that id is actually here.
    let project = bundle
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| matches!(projects.get(p), Ok(Some(_))))
        .map(str::to_string);
    if let Err(e) = write_manifest(
        &dir,
        &Manifest {
            name: Some(name.to_string()),
            description: bundle.description.as_deref().map(str::trim).map(str::to_string),
            project,
            // An import is a live dashboard by definition — including one landing on top of a row
            // that had been archived here, which is precisely how you un-retire one.
            archived_at: None,
            moved_to: None,
        },
    ) {
        return error(500, &format!("writing the dashboard manifest: {e}"));
    }

    let host = preferred_host(&dir, name, bundle.host.as_deref());
    if let Err(e) = std::fs::write(dir.join(".adi").join(HIVE_LIVE), hive_yaml(&dir, &host)) {
        return error(500, &format!("writing the dashboard hive file: {e}"));
    }
    // An update may have landed on a dashboard that was archived here, whose hive file is still
    // parked outside the supervisor's glob. Two files would then describe one dashboard.
    let _ = std::fs::remove_file(dir.join(".adi").join(HIVE_ARCHIVED));

    ok_json(&read_dashboard(&dir, ports, live))
}

/// A bundle's files, decoded and resolved to absolute paths under the dashboard directory.
type DecodedFiles = Vec<(PathBuf, Vec<u8>)>;

/// Decode every file's bytes and resolve its path, refusing the whole bundle on the first thing
/// that does not belong.
///
/// The path check is [`adi_fs::Jail`]'s, not one written here: it is the same lexical rule the
/// store browser is confined by, and a second implementation is a second chance to get `..` wrong.
/// On top of it, the generated directories are refused outright — a bundle claiming to carry
/// `.adi/hive.yaml` is a bundle trying to choose this machine's routing.
fn decode_bundle(dir: &Path, files: &[BundleFile]) -> Result<DecodedFiles, Response> {
    let jail = adi_fs::Jail::new(dir);
    let mut decoded = Vec::with_capacity(files.len());
    let mut total = 0_u64;
    for file in files {
        let path = file.path.trim();
        if path.is_empty() {
            return Err(error(400, "the bundle carries a file with no path"));
        }
        if path
            .split(['/', '\\'])
            .any(|segment| NEVER_BUNDLED_DIRS.contains(&segment))
        {
            return Err(error(
                400,
                &format!("a bundle may not carry {path:?} — that directory is generated here"),
            ));
        }
        let resolved = jail
            .resolve(path)
            .map_err(|e| error(400, &format!("refusing {path:?}: {e}")))?;
        if resolved == dir {
            return Err(error(400, &format!("{path:?} names the dashboard itself")));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.contents.as_bytes())
            .map_err(|e| error(400, &format!("{path:?} is not valid base64: {e}")))?;
        total += bytes.len() as u64;
        if total > MAX_BUNDLE_BYTES {
            return Err(error(413, "the bundle is too large"));
        }
        decoded.push((resolved, bytes));
    }
    Ok(decoded)
}

/// Mirror `decoded` into the dashboard directory: drop what an earlier version left behind, then
/// write what this one carries.
fn write_import(dir: &Path, decoded: &DecodedFiles) -> std::io::Result<()> {
    clear_imported(dir)?;
    std::fs::create_dir_all(dir.join(".adi"))?;
    for (path, bytes) in decoded {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

/// Empty a dashboard directory of everything an import replaces, keeping [`KEPT_ON_IMPORT`]. A
/// directory that does not exist yet is simply nothing to clear.
fn clear_imported(dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if KEPT_ON_IMPORT.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        // `symlink_metadata`, so a symlinked directory is unlinked rather than walked into.
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// An id from a bundle, accepted only as one ordinary path segment — it becomes a directory name
/// under the dashboards root, and the far side chose it.
fn valid_id(raw: &str) -> Option<String> {
    let id = raw.trim();
    let usable = !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    usable.then(|| id.to_string())
}

/// The hostname an imported dashboard takes: the one it answered on where it came from, when that
/// label is free here, and a freshly derived one when it is not.
///
/// Keeping the label is what makes a transfer feel like a move rather than a copy — the same
/// dashboard is `nosh.adi` locally and `nosh.<node>.n.adi` through the mesh. But it is only ever a
/// preference: a label another dashboard on this machine already claims would make routing a
/// coin-flip, and a reserved one would shadow infrastructure.
fn preferred_host(dir: &Path, name: &str, offered: Option<&str>) -> String {
    let taken = claimed_labels(dir);
    let offered = offered
        .map(label_of)
        .filter(|label| slugify(label).as_deref() == Some(label.as_str()))
        .filter(|label| !is_reserved(label) && !taken.contains(label));
    match offered {
        Some(label) => format!("{label}.{HOST_ZONE}"),
        None => dashboard_host(dir, name),
    }
}

/// Stand the local copy down once a node has confirmed it holds the dashboard — the second half
/// of a transfer in `move` mode.
///
/// Archiving rather than deleting is the default because the node's copy is now the only other
/// one: the hive file is parked (so both bun servers stop within a few seconds), the manifest
/// records where it went, and Restore is still there if the move turns out to have been a
/// mistake. `delete` is the operator saying they meant it.
///
/// Called only after a `200` from the node, never speculatively.
#[must_use]
pub fn complete_move(
    cfg: &Config,
    ports: &Ports,
    live: &[UsedPort],
    id: &str,
    node: &str,
    delete: bool,
) -> Response {
    let Some(dir) = dashboard_dir(cfg, id) else {
        return error(404, &format!("no such dashboard: {id}"));
    };
    if delete {
        return match std::fs::remove_dir_all(&dir) {
            Ok(()) => dashboards(cfg, ports, live),
            Err(e) => error(
                500,
                &format!("{node} has the dashboard, but the local copy could not be deleted: {e}"),
            ),
        };
    }

    let mut manifest = read_manifest(&dir);
    manifest.archived_at = Some(now_secs());
    manifest.moved_to = Some(node.to_string());
    if let Err(e) = write_manifest(&dir, &manifest) {
        return error(
            500,
            &format!("{node} has the dashboard, but the local manifest could not be updated: {e}"),
        );
    }
    // Out of the supervisor's `**/hive.yaml` glob, which is what actually stops the two servers.
    let supervised = dir.join(".adi").join(HIVE_LIVE);
    if supervised.exists()
        && let Err(e) = std::fs::rename(&supervised, dir.join(".adi").join(HIVE_ARCHIVED))
    {
        return error(
            500,
            &format!("{node} has the dashboard, but the local one could not be stopped: {e}"),
        );
    }
    dashboards(cfg, ports, live)
}

/// Quote a value as a TOML basic string, escaping what that grammar requires.
fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Every dashboard, sorted by name, with live ports and running flags. `live` is the machine's
/// listening TCP ports (the host does the platform scan and passes it in).
#[must_use]
pub fn dashboards(cfg: &Config, ports: &Ports, live: &[UsedPort]) -> Response {
    let root = cfg.module("dashboards").dir().to_path_buf();

    let mut dashboards: Vec<Dashboard> = match std::fs::read_dir(&root) {
        Ok(entries) => entries
            .flatten()
            // The supervisor's own `hive.yaml` lives beside the dashboards; only dirs count.
            .filter(|e| e.path().is_dir())
            .map(|e| read_dashboard(&e.path(), ports, live))
            .collect(),
        // No dashboards directory yet is an empty list, not an error.
        Err(_) => Vec::new(),
    };

    dashboards.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    ok_json(&DashboardsState { dashboards })
}

/// Read one dashboard directory into its DTO. Every field degrades independently: a missing
/// manifest, an unleased port, or an absent `modules/` dir each fall back rather than failing
/// the whole listing.
///
/// This is also where a dashboard is brought up to the current contract — see [`migrate`]. Doing
/// it on read is deliberate: dashboards are directories a user can copy in, and the listing is
/// the one code path that visits every one of them.
fn read_dashboard(dir: &Path, ports: &Ports, live: &[UsedPort]) -> Dashboard {
    let id = dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());

    let manifest = read_manifest(dir);
    migrate(dir, manifest.name.as_deref().unwrap_or(&id));

    // The ports manager is the source of truth adi-hive allocated from, so read it rather than
    // the hive.yaml (which deliberately declares no ports).
    let port_of = |service: &str| ports.get(&format!("{id}/{service}"), "http").ok().flatten();
    let frontend_port = port_of("frontend");
    let backend_port = port_of("backend");

    Dashboard {
        name: manifest.name.unwrap_or_else(|| id.clone()),
        description: manifest.description,
        project: manifest.project,
        // Read after [`migrate`], so a dashboard that only just gained a host reports it on the
        // very listing that gave it one, rather than a poll later.
        host: declared_host(dir),
        frontend_running: frontend_port.is_some_and(|p| is_listening(live, p)),
        backend_running: backend_port.is_some_and(|p| is_listening(live, p)),
        frontend_port,
        backend_port,
        modules: ts_stems(&dir.join("frontend").join("modules")),
        routes: ts_stems(&dir.join("backend").join("routes")),
        archived_at: manifest.archived_at,
        moved_to: manifest.moved_to,
        dir: dir.display().to_string(),
        id,
    }
}

/// The `.ts` file stems in `dir`, sorted — the module/route ids the entry points discover at
/// runtime. Dotfiles are skipped, matching what the bun servers themselves ignore.
fn ts_stems(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stems: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            name.strip_suffix(".ts").map(str::to_string)
        })
        .collect();
    stems.sort();
    stems
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dashboards root of this test's own, under the system temp dir — never the user's store.
    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-dashboards-api-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    /// A [`Config`] whose whole store is a scratch dir, plus a ports manager whose registry lives
    /// in it too — so nothing in these tests can read or write the real `~/.adi/mono`.
    fn store(tag: &str) -> (Config, Ports) {
        let root = scratch(tag);
        let ports = Ports::with_config(adi_ports_manager::Config {
            registry_path: root.join("ports").join("registry.json"),
            ..Default::default()
        });
        (Config::with_root(root), ports)
    }

    /// The rule from `docs/fleet.md` §2: `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`. Spelled out here
    /// rather than reused from the implementation, so the tests check the contract, not the code.
    fn is_dns_label(label: &str) -> bool {
        let bytes = label.as_bytes();
        let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        !bytes.is_empty()
            && bytes.len() <= 63
            && alnum(bytes[0])
            && alnum(bytes[bytes.len() - 1])
            && bytes.iter().all(|&b| alnum(b) || b == b'-')
    }

    /// A dashboard exactly as the pre-one-origin scaffold left it: a hive file with no `proxy:`
    /// anywhere, entry points that still resolve and inject the backend's port, and one authored
    /// panel and route to prove migration keeps its hands off them.
    fn legacy_dashboard(root: &Path, id: &str, name: &str) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(dir.join("frontend").join("modules")).expect("frontend dir");
        std::fs::create_dir_all(dir.join("backend").join("routes")).expect("backend dir");
        std::fs::create_dir_all(dir.join(".adi")).expect("hive dir");

        std::fs::write(
            dir.join(".adi").join(HIVE_LIVE),
            legacy_hive_yaml(&dir, None),
        )
        .expect("hive file");
        std::fs::write(
            dir.join("frontend").join("index.ts"),
            "const p = await backendPort(); // reads ports/registry.json\n",
        )
        .expect("frontend entry");
        std::fs::write(
            dir.join("frontend").join("index.html"),
            "<script>const api = `http://127.0.0.1:${config.backendPort}`;</script>\n",
        )
        .expect("shell");
        std::fs::write(
            dir.join("backend").join("index.ts"),
            "const CORS = { \"access-control-allow-origin\": \"*\" };\n",
        )
        .expect("backend entry");
        std::fs::write(
            dir.join("frontend").join("modules").join("mine.ts"),
            "export default () => {};\n",
        )
        .expect("panel");
        std::fs::write(
            dir.join("backend").join("routes").join("mine.ts"),
            "export default () => Response.json({});\n",
        )
        .expect("route");
        write_manifest(
            &dir,
            &Manifest {
                name: Some(name.to_string()),
                ..Manifest::default()
            },
        )
        .expect("manifest");
        dir
    }

    /// The hive file the old scaffold wrote: two services, ports left to the manager, and a
    /// `proxy.host` only where somebody added one by hand.
    fn legacy_hive_yaml(dir: &Path, host: Option<&str>) -> String {
        let proxy = host.map_or_else(String::new, |h| format!("    proxy:\n      host: {h}\n"));
        format!(
            "version: \"1\"\n\nservices:\n  frontend:\n    restart: always\n{proxy}\
             \x20   runner:\n      type: script\n      script:\n\
             \x20       run: bun run frontend/index.ts\n        working_dir: {dir}\n\n\
             \x20 backend:\n    restart: always\n    runner:\n      type: script\n\
             \x20     script:\n        run: bun run backend/index.ts\n        working_dir: {dir}\n",
            dir = dir.display(),
        )
    }

    /// Parse a dashboard's hive file the way adi-hive will.
    fn hive_of(dir: &Path) -> HiveFile {
        parse_hive(dir).expect("hive file parses").1
    }

    // MARK: the host label

    #[test]
    fn a_display_name_becomes_one_lowercase_dns_label() {
        assert_eq!(slugify("NakitYok Status").as_deref(), Some("nakityok-status"));
        assert_eq!(slugify("  My  Dash!!  ").as_deref(), Some("my-dash"));
        assert_eq!(slugify("CRM").as_deref(), Some("crm"));
        assert_eq!(slugify("v2.1 metrics").as_deref(), Some("v2-1-metrics"));
    }

    #[test]
    fn a_name_with_nothing_ascii_in_it_slugs_to_nothing() {
        // Transliterating would invent a hostname nobody chose; the id is the honest fallback.
        for name in ["Панель мониторинга", "ダッシュボード", "—", "  ", "!!!"] {
            assert_eq!(slugify(name), None, "{name}");
        }
    }

    #[test]
    fn a_long_name_is_cut_to_a_valid_label() {
        for name in [
            "a".repeat(200),
            format!("{} status page", "b".repeat(70)),
            format!("{}   ", "c".repeat(63)),
        ] {
            let label = slugify(&name).expect("a label");
            assert!(is_dns_label(&label), "{label:?} from {name:?}");
        }
    }

    #[test]
    fn the_host_label_falls_back_to_the_id_when_the_name_yields_none() {
        let root = scratch("fallback-id");
        let id = "84ddcba0-5aaf-4992-80d7-4fdda4bd6339";
        let label = host_label(&root.join(id), id, "Панель");
        assert_eq!(label, id);
        assert!(is_dns_label(&label));
    }

    #[test]
    fn a_label_another_dashboard_already_claims_falls_back_to_the_id() {
        let root = scratch("collision");
        // The neighbour is on `crm.adi` already — derived or hand-picked, it makes no difference.
        let neighbour = legacy_dashboard(&root, "1111", "CRM");
        std::fs::write(
            neighbour.join(".adi").join(HIVE_LIVE),
            legacy_hive_yaml(&neighbour, Some("crm.adi")),
        )
        .expect("neighbour hive");

        let id = "2222";
        assert_eq!(host_label(&root.join(id), id, "CRM"), id);
        // …while the neighbour itself keeps it: its own host never counts as taken.
        assert_eq!(host_label(&neighbour, "1111", "CRM"), "crm");
    }

    #[test]
    fn reserved_labels_are_never_handed_to_a_dashboard() {
        let root = scratch("reserved");
        // `n` is the mesh zone and `app` the control panel — either would shadow live routing.
        for (id, name) in [("3333", "app"), ("4444", "N"), ("5555", "www")] {
            assert_eq!(host_label(&root.join(id), id, name), id, "{name}");
        }
    }

    #[test]
    fn the_derived_host_is_a_label_under_the_adi_zone() {
        let root = scratch("host");
        let dir = root.join("6666");
        let host = dashboard_host(&dir, "NakitYok Status");
        assert_eq!(host, "nakityok-status.adi");
        assert!(is_dns_label(host.split('.').next().expect("label")));
    }

    // MARK: the generated hive file

    #[test]
    fn the_scaffold_declares_one_origin() {
        let root = scratch("scaffold");
        let dir = root.join("7777");
        scaffold(&dir, "Nosh", "", None).expect("scaffold");

        let hive = hive_of(&dir);
        let frontend = hive.services["frontend"].proxy.as_ref().expect("frontend");
        let backend = hive.services["backend"].proxy.as_ref().expect("backend");
        assert_eq!(frontend.host, "nosh.adi");
        assert_eq!(backend.host, "nosh.adi", "both services share one host");
        assert_eq!(frontend.path, None, "the frontend owns the host's root");
        assert_eq!(backend.path.as_deref(), Some("/api"));
        assert!(is_one_origin(&hive));
    }

    #[test]
    fn the_generated_hive_file_still_leaves_the_ports_to_the_manager() {
        let root = scratch("no-ports");
        let dir = root.join("8888");
        scaffold(&dir, "Nosh", "", None).expect("scaffold");

        let raw = std::fs::read_to_string(dir.join(".adi").join(HIVE_LIVE)).expect("hive file");
        assert!(!raw.contains("ports:"), "{raw}");
        assert!(!raw.contains("rollout:"), "{raw}");
        // The runner shape is what the front door strips and the supervisor runs — unchanged.
        assert!(raw.contains("run: bun run frontend/index.ts"), "{raw}");
        assert!(raw.contains("run: bun run backend/index.ts"), "{raw}");
        assert!(
            raw.contains(&format!("working_dir: {}", dir.display())),
            "{raw}"
        );
    }

    // MARK: the templates the browser gets

    #[test]
    fn the_page_carries_no_address_of_its_own() {
        // The shell is what reaches the browser: an absolute URL or a port in here is the whole
        // bug (`docs/fleet.md` §4) — over the mesh `127.0.0.1` is the *viewer's* machine.
        for needle in ["backendPort", "127.0.0.1", "localhost", "http://"] {
            assert!(
                !FRONTEND_INDEX_HTML.contains(needle),
                "the shell must not mention {needle}"
            );
        }
        assert!(FRONTEND_INDEX_HTML.contains(r#"const api = "/api""#));

        // …and the server that renders it no longer looks a port up to inject one.
        for needle in ["backendPort", "registry.json", "BACKEND_PORT"] {
            assert!(
                !FRONTEND_INDEX_TS.contains(needle),
                "the frontend entry must not mention {needle}"
            );
        }
    }

    #[test]
    fn the_backend_serves_the_prefix_it_claims() {
        assert!(BACKEND_INDEX_TS.contains(r#"const API_PREFIX = "/api""#));
        // Wildcard CORS existed only because the page used to call a different origin.
        assert!(!BACKEND_INDEX_TS.contains(LEGACY_CORS));
    }

    // MARK: migration

    #[test]
    fn migration_rewrites_a_legacy_dashboard_to_one_origin() {
        let root = scratch("migrate");
        let dir = legacy_dashboard(&root, "9999", "NakitYok Status");

        migrate(&dir, "NakitYok Status");

        let hive = hive_of(&dir);
        assert!(is_one_origin(&hive), "still not one origin");
        assert_eq!(
            hive.services["frontend"]
                .proxy
                .as_ref()
                .expect("frontend")
                .host,
            "nakityok-status.adi"
        );
        // The entry points that knew an address are replaced by the current templates.
        let read = |p: PathBuf| std::fs::read_to_string(p).expect("entry point");
        assert_eq!(read(dir.join("frontend").join("index.ts")), FRONTEND_INDEX_TS);
        assert_eq!(
            read(dir.join("frontend").join("index.html")),
            FRONTEND_INDEX_HTML
        );
        assert_eq!(read(dir.join("backend").join("index.ts")), BACKEND_INDEX_TS);
    }

    #[test]
    fn migration_is_idempotent() {
        let root = scratch("idempotent");
        let dir = legacy_dashboard(&root, "aaaa", "Nosh");

        migrate(&dir, "Nosh");
        let hive = std::fs::read_to_string(dir.join(".adi").join(HIVE_LIVE)).expect("hive");

        // Two more passes — as the panel's poll would — must not change a byte, or the supervisor
        // would see a config change (and restart both bun servers) every few seconds.
        migrate(&dir, "Nosh");
        migrate(&dir, "Nosh");
        assert_eq!(
            std::fs::read_to_string(dir.join(".adi").join(HIVE_LIVE)).expect("hive"),
            hive
        );

        // And an entry point that no longer carries the marker is left alone, whatever it says.
        let entry = dir.join("frontend").join("index.ts");
        std::fs::write(&entry, "// mine now\n").expect("hand edit");
        migrate(&dir, "Nosh");
        assert_eq!(
            std::fs::read_to_string(&entry).expect("entry point"),
            "// mine now\n"
        );
    }

    #[test]
    fn no_current_template_still_spells_its_own_migration_marker() {
        // A marker left in the file it migrates *to* would make migration rewrite that entry
        // point on every read — a write per poll, forever.
        assert!(!FRONTEND_INDEX_TS.contains(LEGACY_BACKEND_PORT));
        assert!(!FRONTEND_INDEX_HTML.contains(LEGACY_BACKEND_PORT));
        assert!(!BACKEND_INDEX_TS.contains(LEGACY_CORS));
    }

    #[test]
    fn migration_keeps_a_hand_picked_host() {
        let root = scratch("keep-host");
        let dir = legacy_dashboard(&root, "bbbb", "NakitYok Status");
        // Somebody chose `nosh.adi` by hand; that link is bookmarked, so it must survive.
        std::fs::write(
            dir.join(".adi").join(HIVE_LIVE),
            legacy_hive_yaml(&dir, Some("nosh.adi")),
        )
        .expect("hand-picked hive");

        migrate(&dir, "NakitYok Status");

        let hive = hive_of(&dir);
        assert!(is_one_origin(&hive));
        assert_eq!(
            hive.services["backend"].proxy.as_ref().expect("backend").host,
            "nosh.adi"
        );
    }

    #[test]
    fn migration_never_touches_authored_panels_and_routes() {
        let root = scratch("user-content");
        let dir = legacy_dashboard(&root, "cccc", "Nosh");
        let panel = dir.join("frontend").join("modules").join("mine.ts");
        let route = dir.join("backend").join("routes").join("mine.ts");
        let before = (
            std::fs::read_to_string(&panel).expect("panel"),
            std::fs::read_to_string(&route).expect("route"),
        );

        migrate(&dir, "Nosh");

        assert_eq!(std::fs::read_to_string(&panel).expect("panel"), before.0);
        assert_eq!(std::fs::read_to_string(&route).expect("route"), before.1);
    }

    #[test]
    fn migration_leaves_a_hive_file_it_does_not_recognise_alone() {
        let root = scratch("unknown-hive");
        let dir = legacy_dashboard(&root, "dddd", "Nosh");
        // A third service means somebody built something we would be guessing about.
        let custom = format!(
            "{}\n  worker:\n    runner:\n      type: script\n      script:\n        run: bun run w.ts\n",
            legacy_hive_yaml(&dir, None).trim_end()
        );
        std::fs::write(dir.join(".adi").join(HIVE_LIVE), &custom).expect("custom hive");

        migrate(&dir, "Nosh");

        assert_eq!(
            std::fs::read_to_string(dir.join(".adi").join(HIVE_LIVE)).expect("hive"),
            custom
        );
    }

    #[test]
    fn an_archived_dashboard_migrates_its_parked_hive_file() {
        let root = scratch("archived");
        let dir = legacy_dashboard(&root, "eeee", "Nosh");
        std::fs::rename(
            dir.join(".adi").join(HIVE_LIVE),
            dir.join(".adi").join(HIVE_ARCHIVED),
        )
        .expect("park it");

        migrate(&dir, "Nosh");

        // Restoring it must bring back the current shape, not the one it was archived with.
        assert!(is_one_origin(&hive_of(&dir)));
        assert!(!dir.join(".adi").join(HIVE_LIVE).exists());
    }

    #[test]
    fn listing_dashboards_migrates_them() {
        let (cfg, ports) = store("listing");
        let root = cfg.module("dashboards").dir().to_path_buf();
        std::fs::create_dir_all(&root).expect("dashboards root");
        let dir = legacy_dashboard(&root, "ffff", "Nosh");

        assert_eq!(dashboards(&cfg, &ports, &[]).status, 200);

        assert!(is_one_origin(&hive_of(&dir)));
    }

    // MARK: the host on the wire

    /// The listing, parsed back out of the response the panel actually receives.
    fn listed(cfg: &Config, ports: &Ports) -> Vec<Dashboard> {
        let res = dashboards(cfg, ports, &[]);
        assert_eq!(res.status, 200, "{}", res.body);
        serde_json::from_str::<DashboardsState>(&res.body)
            .expect("dashboards state")
            .dashboards
    }

    #[test]
    fn a_listed_dashboard_reports_the_host_it_declares() {
        let (cfg, ports) = store("host-listed");
        let root = cfg.module("dashboards").dir().to_path_buf();
        std::fs::create_dir_all(&root).expect("dashboards root");
        scaffold(&root.join("1234"), "Nosh", "", None).expect("scaffold");

        let listed = listed(&cfg, &ports);
        assert_eq!(listed.len(), 1);
        // The panel links this, so it must be the same name the hive file claims — the one the
        // front door routes `/api` on.
        assert_eq!(listed[0].host.as_deref(), Some("nosh.adi"));
        assert_eq!(listed[0].host.as_deref(), declared_host(&root.join("1234")).as_deref());
    }

    #[test]
    fn a_dashboard_that_declares_no_host_reports_none() {
        let (cfg, ports) = store("host-absent");
        let root = cfg.module("dashboards").dir().to_path_buf();
        std::fs::create_dir_all(&root).expect("dashboards root");
        let dir = legacy_dashboard(&root, "5678", "Nosh");
        // A hive file the migration will not touch (a third service), so it keeps its hostless
        // shape through the listing — the one case where the panel has nothing to link.
        let custom = format!(
            "{}\n  worker:\n    runner:\n      type: script\n      script:\n        run: bun run w.ts\n",
            legacy_hive_yaml(&dir, None).trim_end()
        );
        std::fs::write(dir.join(".adi").join(HIVE_LIVE), custom).expect("custom hive");

        let listed = listed(&cfg, &ports);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].host, None, "nothing is claimed, so nothing is reported");
        // Absent, not blank: `null` round-trips as `None`, so a link is never built from "".
        let json: serde_json::Value = serde_json::from_str(&dashboards(&cfg, &ports, &[]).body)
            .expect("json");
        assert!(json["dashboards"][0]["host"].is_null(), "{json}");
    }

    #[test]
    fn a_hive_file_that_does_not_parse_leaves_the_host_unknown() {
        let (cfg, ports) = store("host-unparseable");
        let root = cfg.module("dashboards").dir().to_path_buf();
        std::fs::create_dir_all(&root).expect("dashboards root");
        let dir = legacy_dashboard(&root, "9abc", "Nosh");
        // Somebody's half-finished edit. The listing visits every dashboard, so a panic here
        // would take out the whole page, not just this row.
        std::fs::write(dir.join(".adi").join(HIVE_LIVE), "services: [oh no\n  : :\n")
            .expect("broken hive");

        let listed = listed(&cfg, &ports);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].host, None);
        assert_eq!(listed[0].name, "Nosh", "the rest of the row still reports");
    }

    #[test]
    fn the_host_field_is_optional_on_the_wire() {
        // The wasm client and adi-app ship apart, so a payload written before this field existed
        // must still deserialize — as "no routable name", which is exactly what it meant.
        let older = r#"{"id":"1","dir":"/d","name":"Nosh","frontend_running":false,
            "backend_running":false,"modules":[],"routes":[]}"#;
        let parsed: Dashboard = serde_json::from_str(older).expect("older payload");
        assert_eq!(parsed.host, None);
    }

    // MARK: transferring a dashboard to another machine

    /// A whole machine of its own: a store, a ports registry, and the [`Projects`] handle the
    /// import side takes. Two of these in one test is a transfer.
    fn machine(tag: &str) -> (Projects, Ports) {
        let (cfg, ports) = store(tag);
        std::fs::create_dir_all(cfg.module("dashboards").dir()).expect("dashboards root");
        (Projects::with_config(cfg), ports)
    }

    /// The dashboards root of a machine.
    fn root_of(projects: &Projects) -> PathBuf {
        projects.config().module("dashboards").dir().to_path_buf()
    }

    /// One bundled file's bytes, by path — `None` when the bundle does not carry it.
    fn bundled(bundle: &DashboardBundle, path: &str) -> Option<Vec<u8>> {
        bundle
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| base64::engine::general_purpose::STANDARD.decode(&f.contents).expect("base64"))
    }

    /// Import `bundle` into `projects`, asserting the node accepted it, and answer with the row it
    /// reported.
    fn import(projects: &Projects, ports: &Ports, bundle: &DashboardBundle) -> Dashboard {
        let body = serde_json::to_vec(bundle).expect("a bundle serializes");
        let res = import_dashboard(projects, ports, &[], &body);
        assert_eq!(res.status, 200, "{}", res.body);
        serde_json::from_str(&res.body).expect("the imported dashboard")
    }

    /// A scaffolded dashboard with one authored panel and one non-UTF-8 asset — the two things a
    /// transfer has to carry that the templates do not.
    fn authored(projects: &Projects, id: &str, name: &str) -> PathBuf {
        let dir = root_of(projects).join(id);
        scaffold(&dir, name, "what it is for", None).expect("scaffold");
        std::fs::write(
            dir.join("frontend").join("modules").join("mine.ts"),
            "export default () => 42;\n",
        )
        .expect("panel");
        std::fs::write(dir.join("frontend").join("logo.png"), [0x89, b'P', 0x00, 0xff])
            .expect("asset");
        dir
    }

    #[test]
    fn a_bundle_carries_what_was_authored_and_nothing_that_was_generated() {
        let (projects, _ports) = machine("bundle");
        let dir = authored(&projects, "d1", "Nosh");
        // Caches nobody should ship, and a symlink that would otherwise put a file from outside
        // the dashboard on the wire.
        std::fs::create_dir_all(dir.join("node_modules").join("left-pad")).expect("cache");
        std::fs::write(dir.join("node_modules").join("left-pad").join("i.js"), "x").expect("dep");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", dir.join("hosts.link")).expect("symlink");

        let bundle = export_bundle(projects.config(), "d1").expect("a bundle");

        assert_eq!(bundle.id, "d1", "the id rides along so a re-transfer updates");
        assert_eq!(bundle.name, "Nosh");
        assert_eq!(bundle.description.as_deref(), Some("what it is for"));
        assert_eq!(bundle.host.as_deref(), Some("nosh.adi"), "offered as a preference");

        assert_eq!(
            bundled(&bundle, "frontend/modules/mine.ts").as_deref(),
            Some(b"export default () => 42;\n".as_slice()),
        );
        // Base64 and not text, so the bytes that are not UTF-8 survive the trip intact.
        assert_eq!(
            bundled(&bundle, "frontend/logo.png").as_deref(),
            Some([0x89, b'P', 0x00, 0xff].as_slice()),
        );

        let paths: Vec<&str> = bundle.files.iter().map(|f| f.path.as_str()).collect();
        for generated in [".adi/hive.yaml", "config.toml"] {
            assert!(!paths.contains(&generated), "{generated} must be rebuilt, not shipped: {paths:?}");
        }
        assert!(
            !paths.iter().any(|p| p.starts_with("node_modules/")),
            "a cache is not part of the dashboard: {paths:?}"
        );
        assert!(
            !paths.contains(&"hosts.link"),
            "a symlink out of the dashboard must never be followed onto the wire: {paths:?}"
        );
        assert_eq!(export_bundle(projects.config(), "nope").err().map(|e| e.status), Some(404));
    }

    #[test]
    fn a_transfer_arrives_as_a_working_dashboard_on_the_other_machine() {
        let (here, _) = machine("transfer-from");
        let (there, there_ports) = machine("transfer-to");
        authored(&here, "d2", "Nosh");

        let bundle = export_bundle(here.config(), "d2").expect("a bundle");
        let landed = import(&there, &there_ports, &bundle);

        // Same dashboard, this machine's paths.
        assert_eq!(landed.id, "d2");
        assert_eq!(landed.name, "Nosh");
        assert_eq!(landed.host.as_deref(), Some("nosh.adi"), "the label was free here");
        let dir = root_of(&there).join("d2");
        assert_eq!(landed.dir, dir.display().to_string());
        assert_eq!(landed.modules, ["mine", "status"], "the panels came across");
        assert_eq!(landed.routes, ["status"]);
        assert!(landed.archived_at.is_none(), "an import is live by definition");

        // The hive file is this machine's own — one origin, and a working_dir under *its* store.
        let hive = hive_of(&dir);
        assert!(is_one_origin(&hive));
        let raw = std::fs::read_to_string(dir.join(".adi").join(HIVE_LIVE)).expect("hive");
        assert!(raw.contains(&format!("working_dir: {}", dir.display())), "{raw}");
        assert!(!raw.contains("transfer-from"), "no path from the sending machine: {raw}");

        // Every authored byte, including the one that is not text.
        assert_eq!(
            std::fs::read(dir.join("frontend").join("logo.png")).expect("asset"),
            [0x89, b'P', 0x00, 0xff],
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("frontend").join("modules").join("mine.ts"))
                .expect("panel"),
            "export default () => 42;\n",
        );
    }

    #[test]
    fn transferring_the_same_dashboard_again_updates_the_copy() {
        let (here, _) = machine("redeploy-from");
        let (there, there_ports) = machine("redeploy-to");
        let from = authored(&here, "d3", "Nosh");
        let to = root_of(&there).join("d3");

        import(&there, &there_ports, &export_bundle(here.config(), "d3").expect("bundle"));
        // Something the node installed for itself, and a stale panel the next transfer drops.
        std::fs::create_dir_all(to.join("node_modules")).expect("cache dir");
        std::fs::write(to.join("node_modules").join("dep.js"), "cached").expect("cache");

        // Edit here: one panel changes, another is deleted.
        std::fs::write(from.join("frontend").join("modules").join("mine.ts"), "// v2\n")
            .expect("edit");
        std::fs::remove_file(from.join("frontend").join("modules").join("status.ts"))
            .expect("delete a panel");

        let again = import(&there, &there_ports, &export_bundle(here.config(), "d3").expect("bundle"));

        // One dashboard, not two — this is what makes "transfer" double as "redeploy".
        let listed = listed(there.config(), &there_ports);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(again.host.as_deref(), Some("nosh.adi"), "the address did not move");
        assert_eq!(
            std::fs::read_to_string(to.join("frontend").join("modules").join("mine.ts"))
                .expect("panel"),
            "// v2\n",
        );
        assert!(
            !to.join("frontend").join("modules").join("status.ts").exists(),
            "a mirror, not a merge: a panel deleted here stops being served there"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("node_modules").join("dep.js")).expect("cache"),
            "cached",
            "the node's install survives, or every re-transfer would be an install"
        );
    }

    #[test]
    fn an_import_over_an_archived_dashboard_brings_it_back_live() {
        let (here, _) = machine("revive-from");
        let (there, there_ports) = machine("revive-to");
        authored(&here, "d4", "Nosh");
        let bundle = export_bundle(here.config(), "d4").expect("bundle");
        import(&there, &there_ports, &bundle);

        // Archived over there — its hive file parked outside the supervisor's glob.
        let res = archive_dashboard(there.config(), &there_ports, &[], br#"{"id":"d4"}"#);
        assert_eq!(res.status, 200, "{}", res.body);
        let to = root_of(&there).join("d4");
        assert!(to.join(".adi").join(HIVE_ARCHIVED).exists());

        let landed = import(&there, &there_ports, &bundle);

        assert!(landed.archived_at.is_none(), "sending it again is how you un-retire it");
        assert!(to.join(".adi").join(HIVE_LIVE).exists(), "supervised again");
        assert!(
            !to.join(".adi").join(HIVE_ARCHIVED).exists(),
            "two hive files would describe one dashboard, and the glob would pick the wrong one"
        );
    }

    #[test]
    fn an_import_refuses_a_path_that_does_not_belong_to_the_dashboard() {
        let (there, there_ports) = machine("escape");
        let root = root_of(&there);

        for path in [
            "../../../../etc/passwd",
            "/etc/passwd",
            "frontend/../../escaped.ts",
            ".adi/hive.yaml",
            "node_modules/dep.js",
            "",
        ] {
            let bundle = DashboardBundle {
                id: "d5".to_string(),
                name: "Nosh".to_string(),
                description: None,
                project: None,
                host: None,
                files: vec![BundleFile {
                    path: path.to_string(),
                    contents: base64::engine::general_purpose::STANDARD.encode("pwned"),
                }],
            };
            let body = serde_json::to_vec(&bundle).expect("serialize");
            let res = import_dashboard(&there, &there_ports, &[], &body);
            assert_eq!(res.status, 400, "{path:?} was accepted: {}", res.body);
        }

        // Refused before anything is written: not a byte of the rejected bundle landed, and no
        // dashboard directory was created for it either.
        assert!(!root.join("d5").exists(), "a refused import left a directory behind");
        assert!(!root.parent().is_some_and(|p| p.join("escaped.ts").exists()));

        // And an id that is not one path segment is refused just as flatly. (The empty file list
        // here is beside the point — an unusable id is refused before it is looked at.)
        for id in ["../elsewhere", "a/b", "", "."] {
            let bundle = DashboardBundle {
                id: id.to_string(),
                name: "Nosh".to_string(),
                description: None,
                project: None,
                host: None,
                files: Vec::new(),
            };
            let body = serde_json::to_vec(&bundle).expect("serialize");
            let res = import_dashboard(&there, &there_ports, &[], &body);
            assert_eq!(res.status, 400, "id {id:?} was accepted: {}", res.body);
        }
    }

    #[test]
    fn an_empty_bundle_never_empties_a_dashboard_that_is_running_here() {
        let (here, _) = machine("empty-from");
        let (there, there_ports) = machine("empty-to");
        authored(&here, "da", "Nosh");
        let mut bundle = export_bundle(here.config(), "da").expect("bundle");
        import(&there, &there_ports, &bundle);

        // An import is a mirror; an empty one would mirror nothing over a live dashboard.
        bundle.files.clear();
        let body = serde_json::to_vec(&bundle).expect("serialize");
        let res = import_dashboard(&there, &there_ports, &[], &body);

        assert_eq!(res.status, 400, "{}", res.body);
        let to = root_of(&there).join("da");
        assert!(to.join("frontend").join("index.ts").exists(), "the copy over there is intact");
        assert!(to.join("frontend").join("modules").join("mine.ts").exists());
    }

    #[test]
    fn an_offered_host_gives_way_to_one_this_machine_already_uses() {
        let (here, _) = machine("host-clash-from");
        let (there, there_ports) = machine("host-clash-to");
        authored(&here, "d6", "Nosh");
        // A different dashboard is already `nosh.adi` on the receiving machine.
        scaffold(&root_of(&there).join("resident"), "Nosh", "", None).expect("neighbour");

        let landed = import(
            &there,
            &there_ports,
            &export_bundle(here.config(), "d6").expect("bundle"),
        );

        assert_eq!(
            landed.host.as_deref(),
            Some("d6.adi"),
            "the offered label was taken, so it falls back to the id — a working hostname beats \
             a pretty one, and two dashboards on one host is a coin-flip",
        );
        // The resident keeps what it had; nothing was quietly re-pointed underneath it.
        assert_eq!(declared_host(&root_of(&there).join("resident")).as_deref(), Some("nosh.adi"));
    }

    #[test]
    fn a_project_id_that_means_nothing_here_leaves_the_copy_unfiled() {
        let (here, _) = machine("project-from");
        let (there, there_ports) = machine("project-to");
        let dir = authored(&here, "d7", "Nosh");
        let mut manifest = read_manifest(&dir);
        manifest.project = Some("a-project-only-over-there".to_string());
        write_manifest(&dir, &manifest).expect("file it");

        let landed = import(
            &there,
            &there_ports,
            &export_bundle(here.config(), "d7").expect("bundle"),
        );

        assert_eq!(landed.project, None, "an id nothing here answers to is not a filing");
    }

    #[test]
    fn a_move_archives_the_local_copy_and_says_where_it_went() {
        let (here, ports) = machine("moved");
        let dir = authored(&here, "d8", "Nosh");

        let res = complete_move(here.config(), &ports, &[], "d8", "laptop-b", false);
        assert_eq!(res.status, 200, "{}", res.body);

        let row = &serde_json::from_str::<DashboardsState>(&res.body).expect("state").dashboards[0];
        assert!(row.is_archived(), "the local one stops running");
        assert_eq!(row.moved_to.as_deref(), Some("laptop-b"));
        // Parked out of the supervisor's glob — which is what actually stops the two bun servers.
        assert!(dir.join(".adi").join(HIVE_ARCHIVED).exists());
        assert!(!dir.join(".adi").join(HIVE_LIVE).exists());
        // Nothing was deleted: Restore is still the way back.
        assert!(dir.join("frontend").join("modules").join("mine.ts").exists());

        // …and restoring drops the note, because this machine runs it again.
        let res = unarchive_dashboard(here.config(), &ports, &[], br#"{"id":"d8"}"#);
        assert_eq!(res.status, 200, "{}", res.body);
        let row = &serde_json::from_str::<DashboardsState>(&res.body).expect("state").dashboards[0];
        assert!(!row.is_archived());
        assert_eq!(row.moved_to, None, "a live dashboard does not live somewhere else");
    }

    #[test]
    fn a_move_that_asked_for_it_deletes_the_local_directory() {
        let (here, ports) = machine("moved-deleted");
        let dir = authored(&here, "d9", "Nosh");

        let res = complete_move(here.config(), &ports, &[], "d9", "laptop-b", true);
        assert_eq!(res.status, 200, "{}", res.body);

        assert!(!dir.exists(), "the operator asked for the local copy to go");
        assert!(
            serde_json::from_str::<DashboardsState>(&res.body).expect("state").dashboards.is_empty()
        );
        // A second attempt has nothing to stand down, and says so rather than reporting success.
        assert_eq!(
            complete_move(here.config(), &ports, &[], "d9", "laptop-b", true).status,
            404
        );
    }

    #[test]
    fn creating_a_dashboard_writes_the_current_shape() {
        let (cfg, ports) = store("create");
        let body = br#"{"name":"Nosh","description":"a dashboard"}"#;

        let res = create_dashboard(&cfg, &ports, body);
        assert_eq!(res.status, 200, "{}", res.body);

        let created: Dashboard = serde_json::from_str(&res.body).expect("dashboard DTO");
        let dir = PathBuf::from(&created.dir);
        assert!(is_one_origin(&hive_of(&dir)));
        // The create response is a row the panel renders straight away, so it carries the host too.
        assert_eq!(created.host.as_deref(), Some("nosh.adi"));
        assert_eq!(
            declared_host(&dir).as_deref(),
            Some("nosh.adi"),
            "the host comes from the name"
        );
    }
}
