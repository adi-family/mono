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

use std::path::{Path, PathBuf};

use adi_config::Config;
use adi_dashboards::{
    BundleError, CollectError, DashboardBundle, HIVE_ARCHIVED, HIVE_LIVE, MAX_BUNDLE_FILES,
    Manifest, dashboard_host, declared_host, decode_bundle, hive_yaml, is_one_origin, parse_hive,
    preferred_host, read_manifest, valid_id, write_import, write_manifest,
};
use adi_ports_manager::Ports;
use adi_projects::Projects;

use crate::types::{
    Dashboard, DashboardRef, DashboardsState, NewDashboard, SetDashboardProject, UsedPort,
};

use super::response::{FromBody, Response, error, ok_json};
use super::services::is_listening;

/// The scaffold a new dashboard starts from — the two fixed entry points plus one worked
/// example of each extension point, embedded so the binary can create a dashboard anywhere.
const FRONTEND_INDEX_TS: &str = include_str!("../../templates/dashboard/frontend/index.ts");
/// The dashboard shell, with `design/tokens.css` spliced in at its `/* @adi-tokens */` marker
/// at build time. The shell has to stay self-contained — it is served under a name of its own,
/// with no adi stylesheet to link — but a copy of the palette here would drift from the one
/// everything else draws from, so it is the file itself, inlined.
static FRONTEND_INDEX_HTML: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    include_str!("../../templates/dashboard/frontend/index.html").replacen(
        "/* @adi-tokens */",
        include_str!("../../../../design/tokens.css").trim_end(),
        1,
    )
});
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

    let module = cfg.module("dashboards");
    // The id is a slug of the name, not a UUID: it is the directory, the port-lease key, and what
    // a manifest would have to name to install this dashboard on another machine — and a UUID is
    // unusable as the last of those and expensive as the first two. See `adi_config::mint`.
    let aliases = adi_config::Aliases::load(&module).unwrap_or_default();
    let id = adi_config::mint(name, ID_FALLBACK, |candidate| {
        module.dir().join(candidate).exists() || aliases.is_alias(candidate)
    });
    let dir = module.dir().join(&id);
    let project = req
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
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
    let req = require!(body, DashboardRef);
    let id = req.id.trim();
    let Some(dir) = dashboard_dir(cfg, id) else {
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
    let req = require!(body, DashboardRef);
    let id = req.id.trim();
    let Some(dir) = dashboard_dir(cfg, id) else {
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
///
/// An id a dashboard no longer has still answers, through the module's alias index
/// ([`adi_config::Aliases`]): this is the one place a dashboard id is turned into a directory, so
/// putting the lookup here is what would make a rename safe for every caller at once. Consulted
/// only on a miss, so the common case is the single `is_dir` it always was.
fn dashboard_dir(cfg: &Config, id: &str) -> Option<PathBuf> {
    let id = id.trim();
    if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\\') {
        return None;
    }
    let module = cfg.module("dashboards");
    let dir = module.dir().join(id);
    if dir.is_dir() {
        return Some(dir);
    }
    let current = module
        .dir()
        .join(adi_config::Aliases::load(&module).ok()?.target(id)?);
    current.is_dir().then_some(current)
}

/// Parse a [`DashboardRef`] body into its trimmed, non-empty id.
impl FromBody for DashboardRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
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
    std::fs::write(
        dir.join("frontend").join("index.html"),
        FRONTEND_INDEX_HTML.as_str(),
    )?;
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

/// The word a dashboard's id falls back to when its name has nothing sluggable in it.
const ID_FALLBACK: &str = "dashboard";

// MARK: migration — replacing generated files with the current templates

/// The generation stamp each generated entry point carries, near the top of its own file. Bump one
/// when you change the template beside it, and every dashboard picks the change up on the next
/// listing.
///
/// A stamp names what the **current** template has, and [`restamp_entry_point`] migrates on its
/// *absence*. That inversion is the whole mechanism. The alternative — a marker naming what the
/// old file had — needs a fresh constant and a fresh migration call per change, and worse, cannot
/// catch a file that a *previous* migration wrote, so the second change in a row never lands
/// anywhere. One stamp covers every generation there has ever been, including the ones migration
/// itself produced, and including files written before stamps existed at all.
const SHELL_STAMP: &str = "<!-- adi-shell: 3";
const FRONTEND_ENTRY_STAMP: &str = "// adi-frontend-entry: 1";
const BACKEND_ENTRY_STAMP: &str = "// adi-backend-entry: 1";

/// Bring a dashboard's generated files up to the current templates, in place, the next time it is
/// read or listed. There is no separate migration command: a dashboard is a directory, and the
/// listing is the only thing guaranteed to visit every one of them.
///
/// Idempotent by construction — every step tests what is on disk and writes only when it is behind,
/// so the panel's few-second poll writes nothing once a dashboard is current, and the supervisor
/// sees no spurious config change.
///
/// It rewrites **generated** files only: the hive file and the three fixed entry points. The panels
/// and routes under `frontend/modules/` and `backend/routes/` are what a user or an agent authored,
/// and are never read here, let alone written.
fn migrate(dir: &Path, name: &str) {
    migrate_hive(dir, name);
    let frontend = dir.join("frontend");
    restamp_entry_point(
        &frontend.join("index.ts"),
        FRONTEND_ENTRY_STAMP,
        FRONTEND_INDEX_TS,
    );
    restamp_entry_point(
        &frontend.join("index.html"),
        SHELL_STAMP,
        &FRONTEND_INDEX_HTML,
    );
    restamp_entry_point(
        &dir.join("backend").join("index.ts"),
        BACKEND_ENTRY_STAMP,
        BACKEND_INDEX_TS,
    );
}

/// Replace one generated entry point with the current template while it does not spell `stamp`.
/// Idempotent: the stamp is there once the first pass has run.
///
/// It replaces a hand-edited entry point rather than leaving it be, which is the documented
/// contract for these three files — the scaffold README marks each "do not edit", and everything a
/// dashboard actually does lives in `frontend/modules/` and `backend/routes/`, which migration
/// never reads.
fn restamp_entry_point(path: &Path, stamp: &str, template: &str) {
    let Ok(current) = std::fs::read_to_string(path) else {
        return;
    };
    if current.contains(stamp) {
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

// MARK: moving a dashboard to another machine

/// Pack a dashboard's authored files into a [`DashboardBundle`] ready to POST at another machine.
///
/// Everything a person or an agent wrote travels; everything a machine generated does not. The
/// packing rules themselves are [`adi_dashboards`]'s — this is only the HTTP-shaped error mapping.
///
/// # Errors
/// The [`Response`] to answer with: 404 for an unknown id, 413 when the directory is past the
/// bundle caps, 500 on a read failure.
pub fn export_bundle(cfg: &Config, id: &str) -> Result<DashboardBundle, Response> {
    let Some(dir) = dashboard_dir(cfg, id) else {
        return Err(error(404, &format!("no such dashboard: {id}")));
    };
    let manifest = read_manifest(&dir);
    let name = manifest.name.clone().unwrap_or_else(|| id.to_string());

    let mut files = Vec::new();
    let mut total = 0_u64;
    if let Err(e) = adi_dashboards::collect_files(&dir, &mut PathBuf::new(), &mut files, &mut total)
    {
        return Err(match e {
            CollectError::TooLarge { .. } => error(413, &e.to_string()),
            CollectError::Io(e) => error(500, &format!("reading {}: {e}", dir.display())),
        });
    }

    Ok(DashboardBundle {
        id: id.to_string(),
        name,
        description: manifest.description,
        project: manifest.project,
        host: declared_host(&dir),
        files,
    })
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
        Err(e) => return bundle_refusal(&e),
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
            description: bundle
                .description
                .as_deref()
                .map(str::trim)
                .map(str::to_string),
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

/// Map a bundle refusal onto the HTTP answer a rejected import earns. Every property of the
/// bundle itself is a 400 except the size caps, which are a 413 — the caller can shrink and retry.
fn bundle_refusal(e: &BundleError) -> Response {
    let status = match e {
        BundleError::TooLarge | BundleError::TooManyFiles => 413,
        _ => 400,
    };
    error(status, &e.to_string())
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
        // An app that arrived from a marketplace and was never started carries `archived_at` like
        // an archived dashboard, and means something else entirely by it.
        //
        // Gated on being archived at all, and not only on the record: a dashboard that is live
        // has plainly been started, whatever its record says — which covers both an app started
        // through Restore (which stamps nothing) and a record written before the stamp existed.
        never_started: manifest.archived_at.is_some()
            && adi_marketplace::install::never_started(dir),
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
    use adi_dashboards::BundleFile;
    use base64::Engine as _;

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
    fn hive_of(dir: &Path) -> adi_dashboards::HiveFile {
        parse_hive(dir).expect("hive file parses").1
    }

    // MARK: the host label — the derivation itself and its tests live in `adi-dashboards`; what
    // stays here is what the scaffold writes through it.

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
        // Wildcard CORS existed only because the page used to call a different origin. Under one
        // origin it is not merely unnecessary — it would let any page you visit read this API.
        assert!(!BACKEND_INDEX_TS.contains("access-control-allow-origin"));
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
        assert_eq!(
            read(dir.join("frontend").join("index.ts")),
            FRONTEND_INDEX_TS
        );
        assert_eq!(
            read(dir.join("frontend").join("index.html")),
            *FRONTEND_INDEX_HTML
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

        // A hand-edited entry point is *replaced*, not preserved — the deliberate cost of
        // migrating on the current template's stamp rather than on the old file's shape. These
        // three files are generated ("do not edit" in the scaffold README); what a dashboard
        // actually does lives in modules/ and routes/, which the test below covers.
        let entry = dir.join("frontend").join("index.ts");
        std::fs::write(&entry, "// mine now\n").expect("hand edit");
        migrate(&dir, "Nosh");
        assert_eq!(
            std::fs::read_to_string(&entry).expect("entry point"),
            FRONTEND_INDEX_TS
        );
    }

    #[test]
    fn every_template_spells_its_own_stamp() {
        // Migration keys on the stamp being *present*, so a template that failed to carry its own
        // would be rewritten on every read — a write per poll, forever, and a bun restart with it.
        assert!(FRONTEND_INDEX_TS.contains(FRONTEND_ENTRY_STAMP));
        assert!(FRONTEND_INDEX_HTML.contains(SHELL_STAMP));
        assert!(BACKEND_INDEX_TS.contains(BACKEND_ENTRY_STAMP));
    }

    #[test]
    fn each_generated_entry_point_migrates_on_its_own_stamp() {
        let root = scratch("stamps");
        let dir = legacy_dashboard(&root, "dddd", "Nosh");
        // Three unrelated files, so a stamp that matched the wrong one would show up as a file
        // left behind rather than as a passing test.
        for (path, template) in [
            (dir.join("frontend").join("index.ts"), FRONTEND_INDEX_TS),
            (
                dir.join("frontend").join("index.html"),
                FRONTEND_INDEX_HTML.as_str(),
            ),
            (dir.join("backend").join("index.ts"), BACKEND_INDEX_TS),
        ] {
            std::fs::write(&path, "// some earlier generation\n").expect("stale entry point");
            migrate(&dir, "Nosh");
            assert_eq!(
                std::fs::read_to_string(&path).expect("entry point"),
                template,
                "{} was not brought up to its template",
                path.display()
            );
        }
    }

    #[test]
    fn a_shell_of_any_earlier_generation_is_restamped() {
        let root = scratch("shell-stamp");
        let dir = legacy_dashboard(&root, "cccc", "Nosh");
        let shell = dir.join("frontend").join("index.html");
        // A shell that is current in every *other* respect — one origin already, so the
        // pre-one-origin marker would never have fired on it — and behind only on the shell's
        // own generation. This is the case a legacy-marker migration cannot express.
        std::fs::write(&shell, "<!doctype html><html><head></head></html>\n").expect("old shell");

        migrate(&dir, "Nosh");

        let read = || std::fs::read_to_string(&shell).expect("shell");
        assert_eq!(read(), *FRONTEND_INDEX_HTML);
        assert!(read().contains("adi-editor__frame"), "no iframe drawer");
        assert!(read().contains("adi-pick"), "no element picker");

        // And the poll behind this must not rewrite it a second time.
        migrate(&dir, "Nosh");
        assert_eq!(read(), *FRONTEND_INDEX_HTML);
    }

    #[test]
    fn the_shell_attributes_a_panel_to_the_module_that_drew_it() {
        // The element picker resolves a pick to a source file through this attribute alone, so
        // the shell has to be the thing that writes it — no module sets it, and nothing else
        // knows which module drew which card.
        assert!(FRONTEND_INDEX_HTML.contains("el.dataset.adiModule = id;"));
        assert!(FRONTEND_INDEX_HTML.contains("data-adi-module"));
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
            hive.services["backend"]
                .proxy
                .as_ref()
                .expect("backend")
                .host,
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
        assert_eq!(
            listed[0].host.as_deref(),
            declared_host(&root.join("1234")).as_deref()
        );
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
        assert_eq!(
            listed[0].host, None,
            "nothing is claimed, so nothing is reported"
        );
        // Absent, not blank: `null` round-trips as `None`, so a link is never built from "".
        let json: serde_json::Value =
            serde_json::from_str(&dashboards(&cfg, &ports, &[]).body).expect("json");
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
        std::fs::write(
            dir.join(".adi").join(HIVE_LIVE),
            "services: [oh no\n  : :\n",
        )
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
        bundle.files.iter().find(|f| f.path == path).map(|f| {
            base64::engine::general_purpose::STANDARD
                .decode(&f.contents)
                .expect("base64")
        })
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
        std::fs::write(
            dir.join("frontend").join("logo.png"),
            [0x89, b'P', 0x00, 0xff],
        )
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

        assert_eq!(
            bundle.id, "d1",
            "the id rides along so a re-transfer updates"
        );
        assert_eq!(bundle.name, "Nosh");
        assert_eq!(bundle.description.as_deref(), Some("what it is for"));
        assert_eq!(
            bundle.host.as_deref(),
            Some("nosh.adi"),
            "offered as a preference"
        );

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
            assert!(
                !paths.contains(&generated),
                "{generated} must be rebuilt, not shipped: {paths:?}"
            );
        }
        assert!(
            !paths.iter().any(|p| p.starts_with("node_modules/")),
            "a cache is not part of the dashboard: {paths:?}"
        );
        assert!(
            !paths.contains(&"hosts.link"),
            "a symlink out of the dashboard must never be followed onto the wire: {paths:?}"
        );
        assert_eq!(
            export_bundle(projects.config(), "nope")
                .err()
                .map(|e| e.status),
            Some(404)
        );
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
        assert_eq!(
            landed.host.as_deref(),
            Some("nosh.adi"),
            "the label was free here"
        );
        let dir = root_of(&there).join("d2");
        assert_eq!(landed.dir, dir.display().to_string());
        assert_eq!(landed.modules, ["mine", "status"], "the panels came across");
        assert_eq!(landed.routes, ["status"]);
        assert!(
            landed.archived_at.is_none(),
            "an import is live by definition"
        );

        // The hive file is this machine's own — one origin, and a working_dir under *its* store.
        let hive = hive_of(&dir);
        assert!(is_one_origin(&hive));
        let raw = std::fs::read_to_string(dir.join(".adi").join(HIVE_LIVE)).expect("hive");
        assert!(
            raw.contains(&format!("working_dir: {}", dir.display())),
            "{raw}"
        );
        assert!(
            !raw.contains("transfer-from"),
            "no path from the sending machine: {raw}"
        );

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

        import(
            &there,
            &there_ports,
            &export_bundle(here.config(), "d3").expect("bundle"),
        );
        // Something the node installed for itself, and a stale panel the next transfer drops.
        std::fs::create_dir_all(to.join("node_modules")).expect("cache dir");
        std::fs::write(to.join("node_modules").join("dep.js"), "cached").expect("cache");

        // Edit here: one panel changes, another is deleted.
        std::fs::write(
            from.join("frontend").join("modules").join("mine.ts"),
            "// v2\n",
        )
        .expect("edit");
        std::fs::remove_file(from.join("frontend").join("modules").join("status.ts"))
            .expect("delete a panel");

        let again = import(
            &there,
            &there_ports,
            &export_bundle(here.config(), "d3").expect("bundle"),
        );

        // One dashboard, not two — this is what makes "transfer" double as "redeploy".
        let listed = listed(there.config(), &there_ports);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(
            again.host.as_deref(),
            Some("nosh.adi"),
            "the address did not move"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("frontend").join("modules").join("mine.ts"))
                .expect("panel"),
            "// v2\n",
        );
        assert!(
            !to.join("frontend")
                .join("modules")
                .join("status.ts")
                .exists(),
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

        assert!(
            landed.archived_at.is_none(),
            "sending it again is how you un-retire it"
        );
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
        assert!(
            !root.join("d5").exists(),
            "a refused import left a directory behind"
        );
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
        assert!(
            to.join("frontend").join("index.ts").exists(),
            "the copy over there is intact"
        );
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
        assert_eq!(
            declared_host(&root_of(&there).join("resident")).as_deref(),
            Some("nosh.adi")
        );
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

        assert_eq!(
            landed.project, None,
            "an id nothing here answers to is not a filing"
        );
    }

    #[test]
    fn a_move_archives_the_local_copy_and_says_where_it_went() {
        let (here, ports) = machine("moved");
        let dir = authored(&here, "d8", "Nosh");

        let res = complete_move(here.config(), &ports, &[], "d8", "laptop-b", false);
        assert_eq!(res.status, 200, "{}", res.body);

        let row = &serde_json::from_str::<DashboardsState>(&res.body)
            .expect("state")
            .dashboards[0];
        assert!(row.is_archived(), "the local one stops running");
        assert_eq!(row.moved_to.as_deref(), Some("laptop-b"));
        // Parked out of the supervisor's glob — which is what actually stops the two bun servers.
        assert!(dir.join(".adi").join(HIVE_ARCHIVED).exists());
        assert!(!dir.join(".adi").join(HIVE_LIVE).exists());
        // Nothing was deleted: Restore is still the way back.
        assert!(
            dir.join("frontend")
                .join("modules")
                .join("mine.ts")
                .exists()
        );

        // …and restoring drops the note, because this machine runs it again.
        let res = unarchive_dashboard(here.config(), &ports, &[], br#"{"id":"d8"}"#);
        assert_eq!(res.status, 200, "{}", res.body);
        let row = &serde_json::from_str::<DashboardsState>(&res.body)
            .expect("state")
            .dashboards[0];
        assert!(!row.is_archived());
        assert_eq!(
            row.moved_to, None,
            "a live dashboard does not live somewhere else"
        );
    }

    #[test]
    fn a_move_that_asked_for_it_deletes_the_local_directory() {
        let (here, ports) = machine("moved-deleted");
        let dir = authored(&here, "d9", "Nosh");

        let res = complete_move(here.config(), &ports, &[], "d9", "laptop-b", true);
        assert_eq!(res.status, 200, "{}", res.body);

        assert!(!dir.exists(), "the operator asked for the local copy to go");
        assert!(
            serde_json::from_str::<DashboardsState>(&res.body)
                .expect("state")
                .dashboards
                .is_empty()
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
