//! The `/api/*` server surface: the real backend over the [`adi_ports_manager`] port
//! registry. Each handler returns `(status, json_body)`; the host ([`adi-app`](../adi-app))
//! owns the socket and writes the response. Compiled only with the `server` feature,
//! which pulls in the filesystem-backed registry and so is native-only.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use adi_mesh::config::{Forward, MeshConfig};
use adi_mesh::{identity, ticket};
use adi_ports_manager::Ports;
use adi_projects::{Error as ProjectStoreError, Projects};
use serde::Deserialize;

use crate::types::{
    ApiError, Health, Lease, LeaseRef, MeshForward, MeshForwardRef, MeshListenRef, MeshPeerRef,
    MeshPortRef, MeshState, NewProject, PortsState, Project, ProjectDetail, ProjectRef,
    ProjectService, ProjectsState, Range, ReleaseResponse, ReserveResponse, ServicePort, UsedPort,
    UsedPorts,
};

/// `GET /api/health` — liveness plus identity and uptime. The host supplies its own
/// `service`/`version` so the reported identity is the app's, not this library's.
#[must_use]
pub fn health(service: &str, version: &str, start: Instant) -> (u16, String) {
    ok_json(&Health {
        ok: true,
        service: service.to_string(),
        version: version.to_string(),
        uptime_secs: start.elapsed().as_secs(),
    })
}

/// `GET /api/ports` — the allocator's configuration and current static leases.
#[must_use]
pub fn ports(manager: &Ports) -> (u16, String) {
    let config = manager.config();
    let range = Range {
        start: *config.range.start(),
        end: *config.range.end(),
    };
    let reserved = config
        .reserved
        .iter()
        .map(|band| Range {
            start: *band.start(),
            end: *band.end(),
        })
        .collect();

    match manager.leases() {
        Ok(leases) => {
            let leases = leases
                .into_iter()
                .map(|l| Lease {
                    service: l.service,
                    key: l.key,
                    port: l.port,
                })
                .collect();
            ok_json(&PortsState {
                range,
                reserved,
                leases,
            })
        }
        Err(e) => error(500, &format!("reading port registry: {e}")),
    }
}

/// `GET /api/ports/used` — the machine's listening TCP ports. The system scan is done by
/// the host (it's platform I/O); this just shapes the response.
#[must_use]
pub fn used_ports(ports: Vec<UsedPort>) -> (u16, String) {
    ok_json(&UsedPorts { ports })
}

/// `POST /api/ports/reserve` — reserve (or return the existing) static port for a pair.
#[must_use]
pub fn reserve(manager: &Ports, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_lease_ref(body) else {
        return bad_lease_ref();
    };
    match manager.reserve(&req.service, &req.key) {
        Ok(port) => ok_json(&ReserveResponse {
            service: req.service,
            key: req.key,
            port,
        }),
        Err(e) => error(500, &format!("reserving port: {e}")),
    }
}

/// `POST /api/ports/release` — release a static lease, reporting the freed port.
#[must_use]
pub fn release(manager: &Ports, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_lease_ref(body) else {
        return bad_lease_ref();
    };
    match manager.release(&req.service, &req.key) {
        Ok(freed) => ok_json(&ReleaseResponse {
            service: req.service,
            key: req.key,
            freed,
        }),
        Err(e) => error(500, &format!("releasing port: {e}")),
    }
}

// MARK: projects — metadata manifests under ~/.adi/mono/projects

/// `GET /api/projects` — every registered project. Each mutation endpoint below returns a
/// fresh [`ProjectsState`], so the client refreshes from one round-trip.
#[must_use]
pub fn projects(store: &Projects) -> (u16, String) {
    match store.list() {
        Ok(list) => ok_json(&ProjectsState {
            projects: list.into_iter().map(project_dto).collect(),
        }),
        Err(e) => project_error(&e),
    }
}

/// `POST /api/projects/create` — register a project, then report the fresh list.
#[must_use]
pub fn create_project(store: &Projects, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_new_project(body) else {
        return bad_new_project();
    };
    match store.create(req.id.trim(), req.name, req.description) {
        Ok(_) => projects(store),
        Err(e) => project_error(&e),
    }
}

/// `POST /api/projects/archive` — archive a project (soft delete), then report the fresh list.
#[must_use]
pub fn archive_project(store: &Projects, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_project_ref(body) else {
        return bad_project_ref();
    };
    match store.archive(req.id.trim()) {
        Ok(_) => projects(store),
        Err(e) => project_error(&e),
    }
}

/// `POST /api/projects/unarchive` — restore an archived project, then report the fresh list.
#[must_use]
pub fn unarchive_project(store: &Projects, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_project_ref(body) else {
        return bad_project_ref();
    };
    match store.unarchive(req.id.trim()) {
        Ok(_) => projects(store),
        Err(e) => project_error(&e),
    }
}

/// `GET /api/projects/<id>` — one project's manifest plus the services parsed from its
/// `.adi/hive.yaml` (what's "inside" the project).
#[must_use]
pub fn project_detail(store: &Projects, id: &str) -> (u16, String) {
    let project = match store.get(id) {
        Ok(Some(project)) => project,
        Ok(None) => return error(404, &format!("no such project: {id}")),
        Err(e) => return project_error(&e),
    };
    let (has_hive, services) = match store.hive_path(id) {
        Ok(path) => read_hive_services(&path),
        Err(e) => return project_error(&e),
    };
    ok_json(&ProjectDetail {
        name: project.display_name().to_string(),
        id: project.id,
        description: project.manifest.description,
        created_at: project.manifest.created_at,
        archived_at: project.manifest.archived_at,
        has_hive,
        services,
    })
}

/// `POST /api/projects/remove` — permanently delete a project, then report the fresh list.
#[must_use]
pub fn remove_project(store: &Projects, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_project_ref(body) else {
        return bad_project_ref();
    };
    match store.remove(req.id.trim()) {
        Ok(_) => projects(store),
        Err(e) => project_error(&e),
    }
}

// A read-only view of the subset of adi-hive's `hive.yaml` schema the detail page shows.
// adi-hive owns the authoritative schema (`crates/adi-hive/src/config.rs`); it's a binary
// crate with no lib target, so this mirrors just the fields surfaced here rather than
// pulling its full dependency tree (tokio, …) into this library. Unknown fields are ignored.
#[derive(Deserialize)]
struct HiveDoc {
    #[serde(default)]
    services: BTreeMap<String, HiveService>,
}

#[derive(Deserialize)]
struct HiveService {
    #[serde(default)]
    proxy: Option<HiveProxy>,
    #[serde(default)]
    rollout: Option<HiveRollout>,
    #[serde(default)]
    runner: Option<HiveRunner>,
    #[serde(default)]
    restart: Option<String>,
}

#[derive(Deserialize)]
struct HiveProxy {
    host: String,
}

#[derive(Deserialize)]
struct HiveRollout {
    #[serde(default)]
    recreate: Option<HiveRecreate>,
}

#[derive(Deserialize)]
struct HiveRecreate {
    #[serde(default)]
    ports: BTreeMap<String, u16>,
}

#[derive(Deserialize)]
struct HiveRunner {
    #[serde(default)]
    script: Option<HiveScript>,
}

#[derive(Deserialize)]
struct HiveScript {
    run: String,
}

/// Read a project's `.adi/hive.yaml` into `(has_hive, services)`. A missing file is
/// `(false, [])`; a present-but-unparseable file is `(true, [])` — the project has a hive
/// config, just not one we can summarize.
fn read_hive_services(path: &Path) -> (bool, Vec<ProjectService>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (false, Vec::new());
    };
    let Ok(doc) = serde_yaml_ng::from_str::<HiveDoc>(&raw) else {
        return (true, Vec::new());
    };
    let services = doc
        .services
        .into_iter()
        .map(|(name, svc)| ProjectService {
            name,
            host: svc.proxy.map(|p| p.host),
            ports: svc
                .rollout
                .and_then(|r| r.recreate)
                .map(|r| {
                    r.ports
                        .into_iter()
                        .map(|(key, port)| ServicePort { key, port })
                        .collect()
                })
                .unwrap_or_default(),
            run: svc.runner.and_then(|r| r.script).map(|s| s.run),
            restart: svc.restart,
        })
        .collect();
    (true, services)
}

/// Flatten a stored project into its wire [`Project`] DTO.
fn project_dto(project: adi_projects::Project) -> Project {
    let name = project.display_name().to_string();
    Project {
        id: project.id,
        name,
        description: project.manifest.description,
        created_at: project.manifest.created_at,
        archived_at: project.manifest.archived_at,
    }
}

/// Map a store error to an HTTP status: bad id → 400, duplicate → 409, missing → 404, else 500.
fn project_error(e: &ProjectStoreError) -> (u16, String) {
    let status = match e {
        ProjectStoreError::InvalidId(_) => 400,
        ProjectStoreError::Exists(_) => 409,
        ProjectStoreError::NotFound(_) => 404,
        ProjectStoreError::Config(_) | ProjectStoreError::Io(_) => 500,
    };
    error(status, &e.to_string())
}

fn parse_new_project(body: &[u8]) -> Option<NewProject> {
    let req: NewProject = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty()).then_some(req)
}

fn bad_new_project() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"id\": \"…\", \"name\"?: \"…\", \"description\"?: \"…\" }",
    )
}

fn parse_project_ref(body: &[u8]) -> Option<ProjectRef> {
    let req: ProjectRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty()).then_some(req)
}

fn bad_project_ref() -> (u16, String) {
    error(400, "expected JSON body { \"id\": \"…\" }")
}

// MARK: mesh — peer-to-peer port-forwarding config over the adi-mesh library

/// `GET /api/mesh` — this machine's mesh identity, published ticket, and config. `running`
/// is the host's authoritative view of whether the in-process daemon is up (the host owns
/// the daemon's lifecycle, so it passes this in — the same way `health` takes its identity).
#[must_use]
pub fn mesh(running: bool) -> (u16, String) {
    match mesh_snapshot(running) {
        Ok(state) => ok_json(&state),
        Err(e) => error(500, &e),
    }
}

/// `POST /api/mesh/allow` — expose a local TCP port to peers.
#[must_use]
pub fn mesh_allow(running: bool, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_port_ref(body) else {
        return bad_port_ref();
    };
    mesh_edit(running, |cfg| {
        cfg.allow_port(req.port);
    })
}

/// `POST /api/mesh/deny` — stop exposing a local TCP port.
#[must_use]
pub fn mesh_deny(running: bool, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_port_ref(body) else {
        return bad_port_ref();
    };
    mesh_edit(running, |cfg| {
        cfg.deny_port(req.port);
    })
}

/// `POST /api/mesh/peers/allow` — authorize a peer (ticket or id) for the exposed ports;
/// the canonical id is what gets stored.
#[must_use]
pub fn mesh_allow_peer(running: bool, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_peer_ref(body) else {
        return bad_peer_ref();
    };
    let id = match ticket::target_id(&req.peer) {
        Ok(id) => id.to_string(),
        Err(e) => return error(400, &format!("invalid peer: {e}")),
    };
    mesh_edit(running, move |cfg| {
        cfg.allow_peer(id);
    })
}

/// `POST /api/mesh/peers/deny` — revoke a peer's authorization.
#[must_use]
pub fn mesh_deny_peer(running: bool, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_peer_ref(body) else {
        return bad_peer_ref();
    };
    mesh_edit(running, move |cfg| {
        cfg.deny_peer(&req.peer);
    })
}

/// `POST /api/mesh/forwards/add` — forward a local port to a peer's port.
#[must_use]
pub fn mesh_add_forward(running: bool, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_forward_ref(body) else {
        return error(400, "expected JSON body { listen, peer, port, name? }");
    };
    let id = match ticket::target_id(&req.peer) {
        Ok(id) => id,
        Err(e) => return error(400, &format!("invalid peer: {e}")),
    };
    let name = req
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| default_forward_name(&id.to_string(), req.port));
    let forward = Forward {
        name,
        listen: req.listen,
        peer: req.peer,
        port: req.port,
    };
    mesh_edit(running, move |cfg| {
        cfg.add_forward(forward);
    })
}

/// `POST /api/mesh/forwards/remove` — remove the forward bound to a local port.
#[must_use]
pub fn mesh_remove_forward(running: bool, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_listen_ref(body) else {
        return error(400, "expected JSON body { \"listen\": <port> }");
    };
    mesh_edit(running, move |cfg| {
        cfg.remove_forward(req.listen);
    })
}

/// Build the current mesh state: identity, the daemon's published ticket, config, and the
/// host-supplied `running` flag.
fn mesh_snapshot(running: bool) -> Result<MeshState, String> {
    let id = identity::endpoint_id()
        .map_err(|e| format!("reading mesh identity: {e}"))?
        .to_string();
    let cfg = MeshConfig::load().map_err(|e| format!("reading mesh config: {e}"))?;
    Ok(MeshState {
        id,
        running,
        ticket: ticket::published(),
        allow: cfg.host.allow,
        authorized_peers: cfg.host.authorized_peers,
        forwards: cfg
            .forwards
            .into_iter()
            .map(|f| MeshForward {
                name: f.name,
                listen: f.listen,
                peer: f.peer,
                port: f.port,
            })
            .collect(),
    })
}

/// Load the config, apply `mutate`, save it, and return the fresh [`MeshState`] so the
/// client updates from one round-trip.
fn mesh_edit(running: bool, mutate: impl FnOnce(&mut MeshConfig)) -> (u16, String) {
    let mut cfg = match MeshConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => return error(500, &format!("reading mesh config: {e}")),
    };
    mutate(&mut cfg);
    if let Err(e) = cfg.save() {
        return error(500, &format!("saving mesh config: {e}"));
    }
    mesh(running)
}

fn parse_port_ref(body: &[u8]) -> Option<MeshPortRef> {
    let req: MeshPortRef = serde_json::from_slice(body).ok()?;
    (req.port != 0).then_some(req)
}

fn bad_port_ref() -> (u16, String) {
    error(400, "expected JSON body { \"port\": <1-65535> }")
}

fn parse_peer_ref(body: &[u8]) -> Option<MeshPeerRef> {
    let req: MeshPeerRef = serde_json::from_slice(body).ok()?;
    (!req.peer.trim().is_empty()).then_some(req)
}

fn bad_peer_ref() -> (u16, String) {
    error(400, "expected JSON body { \"peer\": \"<id-or-ticket>\" }")
}

fn parse_forward_ref(body: &[u8]) -> Option<MeshForwardRef> {
    let req: MeshForwardRef = serde_json::from_slice(body).ok()?;
    (req.listen != 0 && req.port != 0 && !req.peer.trim().is_empty()).then_some(req)
}

fn parse_listen_ref(body: &[u8]) -> Option<MeshListenRef> {
    let req: MeshListenRef = serde_json::from_slice(body).ok()?;
    (req.listen != 0).then_some(req)
}

/// A short forward label: the peer id's prefix and the remote port.
fn default_forward_name(peer_id: &str, port: u16) -> String {
    let prefix: String = peer_id.chars().take(8).collect();
    format!("{prefix}:{port}")
}

/// A JSON error body paired with its status.
#[must_use]
pub fn error(status: u16, message: &str) -> (u16, String) {
    let body = serde_json::to_string(&ApiError::new(message))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"internal error"}"#.to_string());
    (status, body)
}

/// Serialize a success payload; a serialization failure degrades to a 500 error body.
fn ok_json<T: serde::Serialize>(value: &T) -> (u16, String) {
    match serde_json::to_string(value) {
        Ok(json) => (200, json),
        Err(e) => error(500, &format!("serializing response: {e}")),
    }
}

fn bad_lease_ref() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"service\": \"…\", \"key\": \"…\" }",
    )
}

fn parse_lease_ref(body: &[u8]) -> Option<LeaseRef> {
    let req: LeaseRef = serde_json::from_slice(body).ok()?;
    if req.service.trim().is_empty() || req.key.trim().is_empty() {
        return None;
    }
    Some(req)
}

#[cfg(test)]
mod tests {
    use adi_ports_manager::Config;
    use serde_json::Value;

    use super::*;

    fn temp_manager() -> Ports {
        // Isolated registry per test so we never touch the real one.
        let path = std::env::temp_dir().join(format!(
            "adi-webapp-api-{}-{:?}/registry.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        Ports::with_config(Config {
            registry_path: path,
            ..Config::default()
        })
    }

    #[test]
    fn health_reports_ok_and_identity() {
        let (status, body) = health("adi-app", "1.2.3", Instant::now());
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["service"], "adi-app");
        assert_eq!(v["version"], "1.2.3");
    }

    #[test]
    fn reserve_then_ports_lists_the_lease() {
        let m = temp_manager();
        let (status, body) = reserve(&m, br#"{"service":"web","key":"http"}"#);
        assert_eq!(status, 200);
        let reserved: Value = serde_json::from_str(&body).unwrap();
        let port = reserved["port"].as_u64().unwrap();

        let (status, body) = ports(&m);
        assert_eq!(status, 200);
        let listed: Value = serde_json::from_str(&body).unwrap();
        let leases = listed["leases"].as_array().unwrap();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0]["service"], "web");
        assert_eq!(leases[0]["port"].as_u64().unwrap(), port);
    }

    #[test]
    fn reserve_is_idempotent_over_the_api() {
        let m = temp_manager();
        let (_, first) = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let (_, again) = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let a: Value = serde_json::from_str(&first).unwrap();
        let b: Value = serde_json::from_str(&again).unwrap();
        assert_eq!(a["port"], b["port"]);
    }

    #[test]
    fn release_frees_the_lease() {
        let m = temp_manager();
        let _ = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let (status, body) = release(&m, br#"{"service":"web","key":"http"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["freed"].is_number());

        let (_, body) = ports(&m);
        let listed: Value = serde_json::from_str(&body).unwrap();
        assert!(listed["leases"].as_array().unwrap().is_empty());
    }

    #[test]
    fn bad_body_is_a_400() {
        let m = temp_manager();
        assert_eq!(reserve(&m, b"not json").0, 400);
        assert_eq!(reserve(&m, br#"{"service":"","key":"x"}"#).0, 400);
    }
}
