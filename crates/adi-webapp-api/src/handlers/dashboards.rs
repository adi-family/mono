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

use std::path::Path;

use adi_config::Config;
use adi_ports_manager::Ports;
use serde::Deserialize;

use crate::types::{Dashboard, DashboardsState, NewDashboard};

use super::response::{Response, error, ok_json};

/// The metadata file each dashboard directory carries.
#[derive(Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
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
    if let Err(e) = scaffold(&dir, name, req.description.as_deref().unwrap_or("").trim()) {
        // A half-written directory would be picked up by the supervisor as a broken service,
        // so clear it rather than leave the tree in a state nobody asked for.
        let _ = std::fs::remove_dir_all(&dir);
        return error(500, &format!("could not create dashboard: {e}"));
    }

    ok_json(&read_dashboard(&dir, ports, &[]))
}

/// Write the full scaffold into `dir`. Any error leaves the caller to clean up.
fn scaffold(dir: &Path, name: &str, description: &str) -> std::io::Result<()> {
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

    std::fs::write(
        dir.join("config.toml"),
        format!(
            "name = {}\ndescription = {}\n",
            toml_string(name),
            toml_string(description)
        ),
    )?;
    std::fs::write(dir.join(".adi").join("hive.yaml"), hive_yaml(dir))?;
    Ok(())
}

/// The dashboard's hive services. No `proxy:` (reached by port, so nothing to route) and no
/// declared ports (adi-hive leases them), leaving `working_dir` as the only generated value.
fn hive_yaml(dir: &Path) -> String {
    let dir = dir.display();
    format!(
        "# Dashboard hive services — run by the per-user supervisor \
         (~/.adi/mono/dashboards/hive.yaml).\n\
         #\n\
         # Neither service declares a `proxy:` host: a dashboard is reached on 127.0.0.1:<port>, \
         so it\n\
         # depends on nothing but its own supervisor — not the root front door, not DNS.\n\
         #\n\
         # Neither port is declared either: adi-hive leases a stable one per service from the \
         ports\n\
         # manager (keyed `<dashboard-id>/frontend` and `<dashboard-id>/backend`) and injects it \
         as\n\
         # $PORT. The leases are idempotent, so the ports survive restarts.\n\
         \n\
         version: \"1\"\n\
         \n\
         services:\n\
         \x20 frontend:\n\
         \x20   restart: always\n\
         \x20   runner:\n\
         \x20     type: script\n\
         \x20     script:\n\
         \x20       run: bun run frontend/index.ts\n\
         \x20       working_dir: {dir}\n\
         \n\
         \x20 backend:\n\
         \x20   restart: always\n\
         \x20   runner:\n\
         \x20     type: script\n\
         \x20     script:\n\
         \x20       run: bun run backend/index.ts\n\
         \x20       working_dir: {dir}\n"
    )
}

/// Quote a value as a TOML basic string, escaping what that grammar requires.
fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Every dashboard, sorted by name, with live ports and running flags. `listening` is the set
/// of currently-listening TCP ports (the host does the platform scan and passes it in).
#[must_use]
pub fn dashboards(cfg: &Config, ports: &Ports, listening: &[u16]) -> Response {
    let root = cfg.module("dashboards").dir().to_path_buf();

    let mut dashboards: Vec<Dashboard> = match std::fs::read_dir(&root) {
        Ok(entries) => entries
            .flatten()
            // The supervisor's own `hive.yaml` lives beside the dashboards; only dirs count.
            .filter(|e| e.path().is_dir())
            .map(|e| read_dashboard(&e.path(), ports, listening))
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
fn read_dashboard(dir: &Path, ports: &Ports, listening: &[u16]) -> Dashboard {
    let id = dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());

    let manifest = std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|raw| toml::from_str::<Manifest>(&raw).ok())
        .unwrap_or_default();

    // The ports manager is the source of truth adi-hive allocated from, so read it rather than
    // the hive.yaml (which deliberately declares no ports).
    let port_of = |service: &str| ports.get(&format!("{id}/{service}"), "http").ok().flatten();
    let frontend_port = port_of("frontend");
    let backend_port = port_of("backend");

    Dashboard {
        name: manifest.name.unwrap_or_else(|| id.clone()),
        description: manifest.description,
        frontend_running: frontend_port.is_some_and(|p| listening.contains(&p)),
        backend_running: backend_port.is_some_and(|p| listening.contains(&p)),
        frontend_port,
        backend_port,
        modules: ts_stems(&dir.join("frontend").join("modules")),
        routes: ts_stems(&dir.join("backend").join("routes")),
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
