//! The `/api/*` server surface: the real backend over the [`adi_ports_manager`] port
//! registry. Each handler returns `(status, json_body)`; the host ([`adi-app`](../adi-app))
//! owns the socket and writes the response. Compiled only with the `server` feature,
//! which pulls in the filesystem-backed registry and so is native-only.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use adi_agents::{AgentManifest, Agents, Error as AgentStoreError};
use adi_fs::{Error as FsError, Jail};
use adi_triggers::{Error as TriggerStoreError, TriggerManifest, Triggers};
use adi_mesh::config::{Forward, MeshConfig};
use adi_mesh::{identity, ticket};
use adi_ports_manager::Ports;
use adi_projects::{Error as ProjectStoreError, Projects};
use adi_tasks::{EffectiveStatus, Error as TaskStoreError, TaskStatus, TaskView, Tasks};
use serde::Deserialize;

use crate::types::{
    AgentBackendOption, AgentDto, AgentFormField, AgentFormFieldKind, AgentFormOption,
    AgentFormSpec, AgentKeys, AgentPeek, AgentRef, AgentRunResult, AgentsState, ApiError,
    DirListing, FileContent, FileEntry, FilesRef,
    Health, HiveService, HiveState, HookAck, Lease, LeaseRef, MeshForward, MeshForwardRef,
    MeshListenRef, MeshPeerRef, MeshPortRef, MeshState, NewProject, NewTask, PortsState, Project,
    ProjectDetail, ProjectRef, ProjectService, ProjectsState, Range, ReleaseResponse,
    ReserveResponse, SaveAgent, SaveTrigger, ServicePort, StartResult, StartService, StopResult,
    TaskRow, TasksState, TriggerDto, TriggerFireResult, TriggerKindOption, TriggerLog, TriggerRef,
    TriggersState, UsedPort, UsedPorts, WriteFile,
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
    match store.create(req.id.trim(), req.name, req.description, req.parent) {
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
/// `.adi/hive.yaml` (what's "inside" the project). `listening` is the set of currently-listening
/// TCP ports (the host scans the platform and passes it), so each service gets a live running flag.
#[must_use]
pub fn project_detail(store: &Projects, id: &str, listening: &[u16]) -> (u16, String) {
    let project = match store.get(id) {
        Ok(Some(project)) => project,
        Ok(None) => return error(404, &format!("no such project: {id}")),
        Err(e) => return project_error(&e),
    };
    let (has_hive, services) = match store.hive_path(id) {
        Ok(path) => read_hive_services(&path, listening),
        Err(e) => return project_error(&e),
    };
    let subprojects = match store.children(id) {
        Ok(children) => children.into_iter().map(project_dto).collect(),
        Err(e) => return project_error(&e),
    };
    ok_json(&ProjectDetail {
        name: project.display_name().to_string(),
        id: project.id,
        description: project.manifest.description,
        parent: project.manifest.parent,
        created_at: project.manifest.created_at,
        archived_at: project.manifest.archived_at,
        has_hive,
        services,
        subprojects,
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

// MARK: tasks — the task tree under ~/.adi/mono/tasks/tasks.json

/// `GET /api/tasks` — the whole task tree as a flat list, ordered by task number so a parent
/// precedes the children created after it. The client nests them into a tree by `parent`.
#[must_use]
pub fn tasks(store: &Tasks) -> (u16, String) {
    match store.list(None, None, None, None) {
        Ok(mut views) => {
            views.sort_by(task_order);
            ok_json(&TasksState {
                tasks: views.iter().map(task_row).collect(),
            })
        }
        Err(e) => task_error(&e),
    }
}

/// `POST /api/tasks/create` — create a task (stored status `open`), then report the fresh tree.
/// Only `title` is required; a given `parent` must be an existing task id.
#[must_use]
pub fn create_task(store: &Tasks, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_new_task(body) else {
        return bad_new_task();
    };
    match store.create(
        req.title.trim().to_string(),
        req.details,
        req.project,
        req.tag,
        req.parent,
    ) {
        Ok(_) => tasks(store),
        Err(e) => task_error(&e),
    }
}

// A read-only view of the subset of adi-hive's `hive.yaml` schema the detail page shows.
// adi-hive owns the authoritative schema (`crates/adi-hive/src/config.rs`); it's a binary
// crate with no lib target, so this mirrors just the fields surfaced here rather than
// pulling its full dependency tree (tokio, …) into this library. Unknown fields are ignored.
#[derive(Deserialize)]
struct HiveDoc {
    #[serde(default)]
    services: BTreeMap<String, YamlService>,
}

#[derive(Deserialize)]
struct YamlService {
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
    /// Values may be literals or `` bash`ports-manager.get('name')` `` commands (preprocessed into
    /// `datacommand:<hash>` placeholders), executed to reserve ports when this view reads the
    /// config (see `adi_ports_manager::preprocess` / `ports_map`).
    #[serde(default, deserialize_with = "adi_ports_manager::ports_map")]
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
    #[serde(default)]
    working_dir: Option<String>,
}

/// Read a project's `.adi/hive.yaml` into `(has_hive, services)`, tagging each service with a live
/// running flag (its primary port is in `listening`). A missing file is `(false, [])`; a
/// present-but-unparseable file is `(true, [])` — the project has a hive config, just not one we
/// can summarize.
fn read_hive_services(path: &Path, listening: &[u16]) -> (bool, Vec<ProjectService>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (false, Vec::new());
    };
    // Rewrite `bash`…`` port commands into valid YAML, then parse with the command table
    // installed so port fields resolve and run their commands on read.
    let (yaml, commands) = adi_ports_manager::preprocess(&raw);
    let Ok(doc) =
        adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str::<HiveDoc>(&yaml))
    else {
        return (true, Vec::new());
    };
    let services = doc
        .services
        .into_iter()
        .map(|(name, svc)| {
            let ports: Vec<ServicePort> = svc
                .rollout
                .and_then(|r| r.recreate)
                .map(|r| {
                    r.ports
                        .into_iter()
                        .map(|(key, port)| ServicePort { key, port })
                        .collect()
                })
                .unwrap_or_default();
            let running = primary_port(&ports).is_some_and(|p| listening.contains(&p));
            ProjectService {
                name,
                host: svc.proxy.map(|p| p.host),
                ports,
                run: svc.runner.and_then(|r| r.script).map(|s| s.run),
                restart: svc.restart,
                running,
            }
        })
        .collect();
    (true, services)
}

// MARK: files — a project's own directory, browsed/edited through an isolated jail

/// The largest text file we'll read into the editor or accept on a write. Keeps a single
/// response/request bounded (project files here are configs — small); a larger file is
/// refused rather than truncated. Comfortably under the server's 1 MiB request-body cap.
const MAX_TEXT_BYTES: u64 = 512 * 1024;

/// `POST /api/projects/files` — list a directory inside a project's own directory, confined to
/// it by the [`adi_fs`] jail (no `..`, no absolute paths, no symlink escape). `path` is relative
/// to the project root (`""` is the root).
#[must_use]
pub fn list_files(store: &Projects, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_files_ref(body) else {
        return bad_files_ref();
    };
    let jail = match project_jail(store, &req.id) {
        Ok(jail) => jail,
        Err(resp) => return resp,
    };
    match jail.list(&req.path) {
        Ok(entries) => {
            let path = normalize_rel(&req.path);
            let parent = parent_rel(&path);
            ok_json(&DirListing {
                id: req.id,
                path,
                parent,
                entries: entries.into_iter().map(file_entry).collect(),
            })
        }
        Err(e) => fs_error(&e),
    }
}

/// `POST /api/projects/file/read` — read one text file inside a project's directory. Binary
/// files and files over [`MAX_TEXT_BYTES`] are refused rather than returned.
#[must_use]
pub fn read_file(store: &Projects, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_files_ref(body) else {
        return bad_files_ref();
    };
    let jail = match project_jail(store, &req.id) {
        Ok(jail) => jail,
        Err(resp) => return resp,
    };
    read_file_content(&jail, &req.id, &req.path)
}

/// `POST /api/projects/file/write` — atomically save one text file inside a project's directory,
/// creating any missing parents within it. Returns the fresh [`FileContent`] (re-read from disk)
/// so the client updates its size/modified in one round-trip.
#[must_use]
pub fn write_file(store: &Projects, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_write_file(body) else {
        return error(
            400,
            "expected JSON body { \"id\": \"…\", \"path\": \"…\", \"content\": \"…\" }",
        );
    };
    if req.content.len() as u64 > MAX_TEXT_BYTES {
        return error(
            413,
            &format!("file too large to save (max {MAX_TEXT_BYTES} bytes)"),
        );
    }
    let jail = match project_jail(store, &req.id) {
        Ok(jail) => jail,
        Err(resp) => return resp,
    };
    if let Err(e) = jail.write(&req.path, req.content.as_bytes()) {
        return fs_error(&e);
    }
    // Re-read so the response carries the authoritative size/modified after the write.
    read_file_content(&jail, &req.id, &req.path)
}

/// Read `rel` as text and shape a [`FileContent`], enforcing the [`MAX_TEXT_BYTES`] cap.
fn read_file_content(jail: &Jail, id: &str, rel: &str) -> (u16, String) {
    let meta = match jail.metadata(rel) {
        Ok(meta) => meta,
        Err(e) => return fs_error(&e),
    };
    if meta.is_dir {
        return error(400, &format!("not a file: {rel}"));
    }
    if meta.size > MAX_TEXT_BYTES {
        return error(
            413,
            &format!(
                "file too large to edit ({} bytes, max {MAX_TEXT_BYTES})",
                meta.size
            ),
        );
    }
    match jail.read_to_string(rel) {
        Ok(content) => ok_json(&FileContent {
            id: id.to_string(),
            path: normalize_rel(rel),
            content,
            size: meta.size,
            modified: meta.modified,
        }),
        Err(e) => fs_error(&e),
    }
}

/// Build a jail rooted at a *registered* project's directory. A path with an unsafe id is a
/// 400; an unregistered id is a 404 (mirroring [`project_detail`]); a store failure is a 500.
fn project_jail(store: &Projects, id: &str) -> Result<Jail, (u16, String)> {
    // Only registered projects are browsable — same existence gate as the detail view.
    match store.get(id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(error(404, &format!("no such project: {id}"))),
        Err(e) => return Err(project_error(&e)),
    }
    let dir = store.project_dir(id).map_err(|e| project_error(&e))?;
    Ok(Jail::new(dir))
}

/// Map a jail [`FsError`] to an HTTP status: an escape/`not-a-file` is a 400, a missing path a
/// 404, a non-UTF-8 (binary) file a 415, and any other I/O error a 500.
fn fs_error(e: &FsError) -> (u16, String) {
    let status = match e {
        FsError::Escape(_) | FsError::NotAFile(_) => 400,
        FsError::NotFound(_) => 404,
        FsError::NotText(_) => 415,
        FsError::Io { .. } => 500,
    };
    error(status, &e.to_string())
}

/// Flatten an [`adi_fs::Entry`] into its wire [`FileEntry`] DTO.
fn file_entry(entry: adi_fs::Entry) -> FileEntry {
    FileEntry {
        name: entry.name,
        is_dir: entry.is_dir,
        is_symlink: entry.is_symlink,
        size: entry.size,
        modified: entry.modified,
    }
}

/// Normalize a jailed relative path to a clean display form: keep only real segments joined by
/// `/`, dropping `.` and redundant separators. `..`/absolute paths never reach here (the jail
/// rejects them first). The project root is the empty string.
fn normalize_rel(rel: &str) -> String {
    Path::new(rel)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The parent of a normalized relative path: `None` at the root, else the path with its last
/// segment removed (a top-level entry's parent is the root, `""`).
fn parent_rel(norm: &str) -> Option<String> {
    if norm.is_empty() {
        return None;
    }
    match norm.rsplit_once('/') {
        Some((head, _)) => Some(head.to_string()),
        None => Some(String::new()),
    }
}

fn parse_files_ref(body: &[u8]) -> Option<FilesRef> {
    let req: FilesRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty()).then_some(req)
}

fn bad_files_ref() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"id\": \"…\", \"path\"?: \"…\" }",
    )
}

fn parse_write_file(body: &[u8]) -> Option<WriteFile> {
    let req: WriteFile = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.path.trim().is_empty()).then_some(req)
}

// MARK: hive — every service across all projects + the global front-door hive

/// The port key that names a service's HTTP port (mirrors adi-hive's `HTTP_PORT_KEY`).
const HTTP_PORT_KEY: &str = "http";

/// The port a service's liveness is judged on: the `http` port, else the sole port, else `None`.
fn primary_port(ports: &[ServicePort]) -> Option<u16> {
    if let Some(p) = ports.iter().find(|p| p.key == HTTP_PORT_KEY) {
        return Some(p.port);
    }
    match ports {
        [only] => Some(only.port),
        _ => None,
    }
}

/// `GET /api/hive` — aggregate every service declared across all projects' `.adi/hive.yaml`
/// plus the global `~/.adi/mono/hive/hive.yaml`, tagged with a live running flag. `listening`
/// is the set of currently-listening TCP ports (the host does the platform scan and passes it).
#[must_use]
pub fn hive(store: &Projects, listening: &[u16]) -> (u16, String) {
    let mut services = Vec::new();

    // The global front-door hive lives in the `hive` module of the same store the projects use.
    let global = store.config().module("hive").raw_path("hive.yaml");
    collect_hive_services(None, &global, listening, &mut services);

    // Each project's own hive.yaml (skips archived? no — show every registered project).
    match store.list() {
        Ok(projects) => {
            for project in projects {
                if let Ok(path) = store.hive_path(&project.id) {
                    collect_hive_services(Some(&project.id), &path, listening, &mut services);
                }
            }
        }
        Err(e) => return project_error(&e),
    }

    ok_json(&HiveState { services })
}

/// Parse one hive.yaml and append its services to `out`, tagged with `project` and a running
/// flag (its primary port is in `listening`).
fn collect_hive_services(
    project: Option<&str>,
    path: &Path,
    listening: &[u16],
    out: &mut Vec<HiveService>,
) {
    let (_has_hive, parsed) = read_hive_services(path, listening);
    for svc in parsed {
        let port = primary_port(&svc.ports);
        let running = port.is_some_and(|p| listening.contains(&p));
        out.push(HiveService {
            project: project.map(str::to_string),
            name: svc.name,
            host: svc.host,
            ports: svc.ports,
            run: svc.run,
            restart: svc.restart,
            primary_port: port,
            running,
        });
    }
}

/// `POST /api/hive/start` — launch a hive service's runner (its `run` command) with the
/// ports-manager-allocated `PORT` injected, in its working directory. The child is detached (its
/// own process group) and its output goes to `<workdir>/server.log`; status then reflects the
/// service's primary port listening.
#[must_use]
pub fn start_service(store: &Projects, body: &[u8]) -> (u16, String) {
    let Ok(req) = serde_json::from_slice::<StartService>(body) else {
        return error(400, "expected JSON body { project?, service }");
    };

    // Resolve the hive.yaml and the default working directory for this target.
    let (hive_path, default_dir) = match &req.project {
        Some(id) => match (store.hive_path(id), store.project_dir(id)) {
            (Ok(hive), Ok(dir)) => (hive, Some(dir)),
            (Err(e), _) | (_, Err(e)) => return project_error(&e),
        },
        None => (store.config().module("hive").raw_path("hive.yaml"), None),
    };

    let Ok(raw) = std::fs::read_to_string(&hive_path) else {
        return error(404, "no hive.yaml for that target");
    };
    // Preprocess so `bash`…`` port commands resolve (and reserve their port) on read.
    let (yaml, commands) = adi_ports_manager::preprocess(&raw);
    let Ok(doc) =
        adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str::<HiveDoc>(&yaml))
    else {
        return error(422, "could not parse the hive.yaml");
    };

    let Some(svc) = doc.services.get(&req.service) else {
        return error(404, &format!("no service `{}` in the hive", req.service));
    };
    let Some(script) = svc.runner.as_ref().and_then(|r| r.script.as_ref()) else {
        return error(
            400,
            &format!("service `{}` has no script runner to start", req.service),
        );
    };

    let port = service_http_port(svc);
    // Don't spawn a doomed process if the port is already taken — the service looks up already.
    if let Some(p) = port
        && !adi_ports_manager::is_bindable(p)
    {
        return error(
            409,
            &format!("service `{}` looks already running on :{p}", req.service),
        );
    }
    let workdir = resolve_workdir(script.working_dir.as_deref(), default_dir.as_deref());
    match spawn_runner(&script.run, &workdir, port) {
        Ok(pid) => ok_json(&StartResult {
            service: req.service,
            port,
            pid,
        }),
        Err(e) => error(500, &format!("starting `{}`: {e}", req.service)),
    }
}

/// `POST /api/hive/stop` {project?, service} — stop a running service by killing whatever listens
/// on its resolved port (the runner was spawned in its own process group; a plain kill on the
/// listener stops it).
#[must_use]
pub fn stop_service(store: &Projects, body: &[u8]) -> (u16, String) {
    let Ok(req) = serde_json::from_slice::<StartService>(body) else {
        return error(400, "expected JSON body { project?, service }");
    };
    let hive_path = match &req.project {
        Some(id) => match store.hive_path(id) {
            Ok(hive) => hive,
            Err(e) => return project_error(&e),
        },
        None => store.config().module("hive").raw_path("hive.yaml"),
    };
    let Ok(raw) = std::fs::read_to_string(&hive_path) else {
        return error(404, "no hive.yaml for that target");
    };
    let (yaml, commands) = adi_ports_manager::preprocess(&raw);
    let Ok(doc) =
        adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str::<HiveDoc>(&yaml))
    else {
        return error(422, "could not parse the hive.yaml");
    };
    let Some(svc) = doc.services.get(&req.service) else {
        return error(404, &format!("no service `{}` in the hive", req.service));
    };
    let Some(port) = service_http_port(svc) else {
        return error(
            400,
            &format!("service `{}` has no port to stop", req.service),
        );
    };
    match kill_listener(port) {
        Ok(()) => ok_json(&StopResult {
            service: req.service,
            port: Some(port),
        }),
        Err(e) => error(500, &format!("stopping `{}`: {e}", req.service)),
    }
}

/// SIGTERM whatever process is listening on `port` (best-effort, via `lsof` + `kill`).
fn kill_listener(port: u16) -> std::io::Result<()> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "pids=$(lsof -ti tcp:{port} -sTCP:LISTEN 2>/dev/null); [ -n \"$pids\" ] && kill $pids || true"
        ))
        .env("PATH", augmented_path())
        .status()?;
    Ok(())
}

/// The service's proxied port: the `http` slot, else the sole declared port, else `None`.
fn service_http_port(svc: &YamlService) -> Option<u16> {
    let ports = svc
        .rollout
        .as_ref()
        .and_then(|r| r.recreate.as_ref())
        .map(|r| &r.ports)?;
    ports
        .get("http")
        .copied()
        .or_else(|| (ports.len() == 1).then(|| *ports.values().next().unwrap()))
}

/// Resolve a runner's working directory: an explicit absolute path as-is, a relative one against
/// the project dir, else the project dir (or the current dir when there is none).
fn resolve_workdir(explicit: Option<&str>, default_dir: Option<&Path>) -> PathBuf {
    match explicit {
        Some(dir) => {
            let p = Path::new(dir);
            if p.is_absolute() {
                p.to_path_buf()
            } else if let Some(base) = default_dir {
                base.join(dir)
            } else {
                p.to_path_buf()
            }
        }
        None => default_dir.map_or_else(|| PathBuf::from("."), Path::to_path_buf),
    }
}

/// Spawn `sh -c "<run>"` detached (its own process group, so it survives an app restart), in
/// `workdir`, with `PORT`/`PORT_HTTP` and an augmented `PATH` so user tools (bun, node, …)
/// resolve under a minimal launchd environment. Output is redirected to `<workdir>/server.log`.
fn spawn_runner(run: &str, workdir: &Path, port: Option<u16>) -> std::io::Result<u32> {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(run)
        .current_dir(workdir)
        .env("PATH", augmented_path())
        .process_group(0)
        .stdin(Stdio::null());
    if let Some(p) = port {
        cmd.env("PORT", p.to_string())
            .env("PORT_HTTP", p.to_string());
    }
    match std::fs::File::create(workdir.join("server.log")) {
        Ok(log) => {
            let errlog = log.try_clone()?;
            cmd.stdout(Stdio::from(log)).stderr(Stdio::from(errlog));
        }
        Err(_) => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    Ok(cmd.spawn()?.id())
}

/// A `PATH` that includes the user's common tool directories, so a runner launched under a
/// minimal launchd environment can still find `bun`, `node`, and Homebrew binaries.
fn augmented_path() -> String {
    let mut parts = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        parts.push(format!("{home}/.bun/bin"));
        parts.push(format!("{home}/.local/bin"));
    }
    parts.push("/opt/homebrew/bin".to_string());
    parts.push("/usr/local/bin".to_string());
    parts.push("/usr/bin".to_string());
    parts.push("/bin".to_string());
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(":")
}

/// Flatten a stored project into its wire [`Project`] DTO.
fn project_dto(project: adi_projects::Project) -> Project {
    let name = project.display_name().to_string();
    Project {
        id: project.id,
        name,
        description: project.manifest.description,
        parent: project.manifest.parent,
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

/// Flatten a store [`TaskView`] into its wire [`TaskRow`] DTO, stringifying the status enums.
fn task_row(view: &TaskView) -> TaskRow {
    let task = &view.task;
    TaskRow {
        id: task.id.clone(),
        title: task.title.clone(),
        details: task.details.clone(),
        status: task_status_label(task.status).to_string(),
        effective: effective_status_label(view.effective).to_string(),
        project: task.project.clone(),
        parent: task.parent.clone(),
        tag: task.tag.clone(),
        assignee: task.assignee.clone(),
        children_total: view.children_total,
        children_open: view.children_open,
        created_at: task.created_at,
        updated_at: task.updated_at,
    }
}

/// The wire label for a stored task status.
fn task_status_label(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Open => "open",
        TaskStatus::Done => "done",
        TaskStatus::Archived => "archived",
    }
}

/// The wire label for a computed effective status.
fn effective_status_label(e: EffectiveStatus) -> &'static str {
    match e {
        EffectiveStatus::Ready => "ready",
        EffectiveStatus::Blocked => "blocked",
        EffectiveStatus::Done => "done",
        EffectiveStatus::Archived => "archived",
    }
}

/// Stable task ordering: by creation time (so a parent precedes children created after it),
/// with the id as a tiebreak. Works across both id schemes (`t<N>` and Jira `<KEY>-<N>`).
fn task_order(a: &TaskView, b: &TaskView) -> std::cmp::Ordering {
    a.task
        .created_at
        .cmp(&b.task.created_at)
        .then_with(|| a.task.id.cmp(&b.task.id))
}

/// Map a task-store error to an HTTP status: missing → 404, bad edit → 400, archived → 409, else 500.
fn task_error(e: &TaskStoreError) -> (u16, String) {
    let status = match e {
        TaskStoreError::NotFound(_) => 404,
        TaskStoreError::ParentMissing(_) | TaskStoreError::Cycle => 400,
        TaskStoreError::ReopenFirst => 409,
        TaskStoreError::Store(_) => 500,
    };
    error(status, &e.to_string())
}

fn parse_new_task(body: &[u8]) -> Option<NewTask> {
    let req: NewTask = serde_json::from_slice(body).ok()?;
    (!req.title.trim().is_empty()).then_some(req)
}

fn bad_new_task() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"title\": \"…\" } with a non-empty title",
    )
}

// MARK: agents — AgentDef definitions under ~/.adi/mono/agents

/// `GET /api/agents` — every registered agent definition. Each mutation endpoint below returns a
/// fresh [`AgentsState`], so the client refreshes from one round-trip.
#[must_use]
pub fn agents(store: &Agents) -> (u16, String) {
    match agents_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => agent_error(&e),
    }
}

/// The full [`AgentsState`]: the stored definitions decorated with live run state (one tmux
/// session listing per call), plus the form schema.
fn agents_state(store: &Agents) -> Result<AgentsState, AgentStoreError> {
    let sessions = adi_agents::running_sessions();
    Ok(AgentsState {
        agents: store
            .list()?
            .into_iter()
            .map(|a| agent_dto(a, &sessions))
            .collect(),
        form: agent_form_spec(),
    })
}

/// `POST /api/agents/run` — launch an agent in its backend (tmux executors only today): the
/// engine CLI starts detached in an `adi-agent-<name>` tmux session. Replies with the attach
/// hint plus fresh state (the new session shows as `running`).
#[must_use]
pub fn run_agent(store: &Agents, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let name = req.name.trim();
    let launch = match store.run(name) {
        Ok(launch) => launch,
        Err(e) => return agent_error(&e),
    };
    match agents_state(store) {
        Ok(state) => ok_json(&AgentRunResult {
            message: format!("Started “{name}” — attach: {}", launch.attach),
            state,
        }),
        Err(e) => agent_error(&e),
    }
}

/// `POST /api/agents/save` — create or update an agent definition (an upsert keyed by `name`),
/// then report the fresh list. `name` and `backend` are required.
#[must_use]
pub fn save_agent(store: &Agents, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_save_agent(body) else {
        return bad_save_agent();
    };
    let name = req.name.trim().to_string();
    let manifest = AgentManifest {
        backend: req.backend.trim().to_string(),
        system_prompt: req.system_prompt,
        tools: req.tools.trim().to_string(),
        model: clean(req.model),
        permission_mode: clean(req.permission_mode),
        temperature: req.temperature,
        max_turns: req.max_turns,
        tags: req
            .tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        starred: req.starred,
        project: clean(req.project),
        extra: clean_extra(req.extra),
        // The store owns the timestamps.
        created_at: 0,
        updated_at: 0,
    };
    match store.save(&name, manifest) {
        Ok(_) => agents(store),
        Err(e) => agent_error(&e),
    }
}

/// `POST /api/agents/delete` — delete an agent definition, then report the fresh list.
#[must_use]
pub fn delete_agent(store: &Agents, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match store.delete(req.name.trim()) {
        Ok(_) => agents(store),
        Err(e) => agent_error(&e),
    }
}

/// `POST /api/agents/peek` — a read-only snapshot of a running agent's tmux pane, for the live
/// view. A registered agent without a live session answers `running: false` (200, not an error);
/// only an unknown name is a 404.
#[must_use]
pub fn peek_agent(store: &Agents, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match get_agent(store, req.name.trim()) {
        Ok(agent) => peek_response(&agent),
        Err(e) => agent_error(&e),
    }
}

/// `POST /api/agents/send-keys` — type into a running agent's tmux session (the interactive
/// half of the live view): `text` is sent literally, then `key` is pressed. Replies with a
/// fresh pane snapshot after a short settle delay, so the sender sees the effect immediately.
#[must_use]
pub fn send_agent_keys(store: &Agents, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_agent_keys(body) else {
        return bad_agent_keys();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return agent_error(&e),
    };
    if let Err(e) = adi_agents::send_keys(&agent.name, &req.text, &req.key) {
        return agent_error(&e);
    }
    // Give the TUI a beat to redraw, so the response snapshot already shows the keystrokes.
    std::thread::sleep(std::time::Duration::from_millis(120));
    peek_response(&agent)
}

/// `POST /api/agents/stop` — kill a running agent's tmux session, then report the fresh list
/// (the agent flips back to stopped/runnable). Idempotent: stopping an already-stopped agent
/// succeeds. Only an unknown name is an error (404).
#[must_use]
pub fn stop_agent(store: &Agents, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return agent_error(&e),
    };
    match adi_agents::stop(&agent.name) {
        Ok(_) => agents(store),
        Err(e) => agent_error(&e),
    }
}

/// Look an agent up, folding "not registered" into [`AgentStoreError::NotFound`] (→ 404).
fn get_agent(store: &Agents, name: &str) -> Result<adi_agents::Agent, AgentStoreError> {
    store
        .get(name)?
        .ok_or_else(|| AgentStoreError::NotFound(name.to_string()))
}

/// The [`AgentPeek`] answer for an agent: its live pane capture, or `running: false` without one.
fn peek_response(agent: &adi_agents::Agent) -> (u16, String) {
    let pane = adi_agents::capture_pane(&agent.name);
    ok_json(&AgentPeek {
        running: pane.is_some(),
        output: pane.unwrap_or_default(),
        attach: format!("tmux attach -t {}", adi_agents::session_name(&agent.name)),
        name: agent.name.clone(),
    })
}

/// Flatten a stored agent into its wire [`AgentDto`], computing the executor and the run state
/// (`runnable` from the backend adapter, `running` from the live tmux `sessions`) for the client.
fn agent_dto(agent: adi_agents::Agent, sessions: &std::collections::BTreeSet<String>) -> AgentDto {
    let executor = agent.manifest.executor().to_string();
    let runnable = adi_agents::is_runnable(&agent.manifest);
    let running = sessions.contains(&adi_agents::session_name(&agent.name));
    let m = agent.manifest;
    AgentDto {
        name: agent.name,
        backend: m.backend,
        executor,
        system_prompt: m.system_prompt,
        tools: m.tools,
        model: m.model,
        permission_mode: m.permission_mode,
        temperature: m.temperature,
        max_turns: m.max_turns,
        tags: m.tags,
        starred: m.starred,
        project: m.project,
        extra: m.extra,
        created_at: m.created_at,
        updated_at: m.updated_at,
        runnable,
        running,
    }
}

/// The agentic-loop backend that picks its model provider at definition time (the `provider`
/// extra); every other backend has its engine baked into the `executor:what` id.
const ADI_HARNESS: &str = "harness:adi";

/// The backends whose engine is the Claude CLI/SDK, whatever the executor.
const CLAUDE_BACKENDS: &[&str] = &["tmux:claude", "process:claude", "harness:claude-sdk"];

/// The backends whose engine is the Codex CLI.
const CODEX_BACKENDS: &[&str] = &["tmux:codex", "process:codex"];

/// Static backend/form metadata for the Agents page. This lives server-side so the API defines
/// both the selectable backends and the field shape the client renders. Backends are
/// `executor:what` pairs — the executor (`tmux` / `process` / `harness`) is the run mechanism,
/// the suffix is what it runs.
#[allow(clippy::too_many_lines)]
fn agent_form_spec() -> AgentFormSpec {
    let mut fields = Vec::new();

    let mut name = agent_field("name", "Name", AgentFormFieldKind::Text);
    name.placeholder = "athz-solver".into();
    name.hint = "a task tagged this name auto-starts it".into();
    name.mono = true;
    name.required = true;
    fields.push(name);

    let mut backend = agent_field("backend", "Backend", AgentFormFieldKind::Select);
    backend.required = true;
    fields.push(backend);

    // The project the agent is filed under (or global). The options are the registered
    // projects, which only the client knows live — it special-cases this field by name and
    // fills the select from its projects state, like the Triggers form does.
    let mut project = agent_field("project", "Project", AgentFormFieldKind::Select);
    project.hint = "shows on that project's page".into();
    fields.push(project);

    // The adi harness runs its own agentic loop and needs to know which provider API to call;
    // provider-specific knobs below are scoped to this choice via `providers`.
    let mut provider =
        field_ids("provider", "Provider", AgentFormFieldKind::Select, &[ADI_HARNESS]);
    provider.options = opts(&[
        ("", "— pick a provider —"),
        ("anthropic", "Anthropic"),
        ("openai", "OpenAI"),
        ("gemini", "Gemini"),
        ("monshoot", "Monshoot"),
        ("ollama", "Ollama (local)"),
    ]);
    provider.hint = "model provider the adi loop calls".into();
    fields.push(provider);

    let mut model = agent_field("model", "Model", AgentFormFieldKind::Text);
    model.placeholder = "model alias".into();
    model.mono = true;
    fields.push(model);

    // ---- claude engines (any executor) ----
    let mut permission =
        field_ids("permission_mode", "Permission mode", AgentFormFieldKind::Select, CLAUDE_BACKENDS);
    permission.options = opts(&[
        ("", "— default —"),
        ("acceptEdits", "acceptEdits"),
        ("auto", "auto"),
        ("bypassPermissions", "bypassPermissions"),
        ("manual", "manual"),
        ("dontAsk", "dontAsk"),
        ("plan", "plan"),
    ]);
    fields.push(permission);

    fields.push(for_providers(
        sel_field(
            "effort",
            "Effort",
            CLAUDE_BACKENDS,
            opts(&[
                ("", "— default —"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("xhigh", "xhigh"),
                ("max", "max"),
            ]),
            "thinking / reasoning depth",
        ),
        &["anthropic"],
    ));

    fields.push(sel_field(
        "output_format",
        "Output format",
        &["process:claude"],
        opts(&[("", "text (default)"), ("json", "json"), ("stream-json", "stream-json")]),
        "how the run result is emitted",
    ));

    let mut allowed = txt_field(
        "allowed_tools",
        "Allowed tools",
        CLAUDE_BACKENDS,
        "Bash(git *) Edit Read",
        "built-in tools to allow",
    );
    allowed.wide = true;
    fields.push(allowed);

    let mut disallowed = txt_field(
        "disallowed_tools",
        "Disallowed tools",
        CLAUDE_BACKENDS,
        "Bash(rm *) WebFetch",
        "built-in tools to deny",
    );
    disallowed.wide = true;
    fields.push(disallowed);

    fields.push(num_field(
        "max_budget_usd",
        "Max budget (USD)",
        &["process:claude"],
        "e.g. 5",
        "hard spend cap (print mode)",
    ));

    fields.push(txt_field(
        "fallback_model",
        "Fallback model",
        CLAUDE_BACKENDS,
        "sonnet",
        "used when the primary model is overloaded",
    ));

    let mut append = field_ids(
        "append_system_prompt",
        "Append system prompt",
        AgentFormFieldKind::Textarea,
        CLAUDE_BACKENDS,
    );
    append.placeholder = "Appended after the default system prompt…".into();
    append.wide = true;
    fields.push(append);

    // ---- codex engines (any executor) ----
    fields.push(sel_field(
        "sandbox",
        "Sandbox",
        CODEX_BACKENDS,
        opts(&[
            ("", "— default —"),
            ("read-only", "read-only"),
            ("workspace-write", "workspace-write"),
            ("danger-full-access", "danger-full-access"),
        ]),
        "filesystem / exec sandbox policy",
    ));

    fields.push(sel_field(
        "approval",
        "Approval",
        CODEX_BACKENDS,
        opts(&[
            ("", "— default —"),
            ("untrusted", "untrusted"),
            ("on-request", "on-request"),
            ("on-failure", "on-failure"),
            ("never", "never"),
        ]),
        "when to ask before running a command",
    ));

    fields.push(for_providers(
        sel_field(
            "reasoning_effort",
            "Reasoning effort",
            CODEX_BACKENDS,
            opts(&[("", "— default —"), ("low", "low"), ("medium", "medium"), ("high", "high")]),
            "reasoning depth",
        ),
        &["openai"],
    ));

    fields.push(txt_field(
        "working_dir",
        "Working dir",
        CODEX_BACKENDS,
        "/path/to/repo",
        "agent working root (-C)",
    ));

    fields.push(chk_field("skip_git_repo_check", "Skip git-repo check", CODEX_BACKENDS));
    fields.push(chk_field("web_search", "Web search", CODEX_BACKENDS));
    fields.push(chk_field("json_events", "JSONL events", &["process:codex"]));

    // ---- tmux/process shared (a vendor CLI runs either way) ----
    let mut add_dir =
        field_executors("add_dir", "Add dir", AgentFormFieldKind::Text, &["tmux", "process"]);
    add_dir.placeholder = "/extra/writable/dir".into();
    add_dir.hint = "additional writable directory".into();
    add_dir.mono = true;
    add_dir.wide = true;
    fields.push(add_dir);

    // ---- harness:adi provider knobs (scoped to the `provider` extra) ----
    fields.push(for_providers(
        sel_field(
            "thinking",
            "Thinking",
            &[],
            opts(&[("", "— default —"), ("adaptive", "adaptive"), ("disabled", "disabled")]),
            "extended-thinking mode",
        ),
        &["anthropic"],
    ));

    fields.push(for_providers(
        num_field("frequency_penalty", "Frequency penalty", &[], "-2.0 – 2.0", ""),
        &["openai"],
    ));
    fields.push(for_providers(
        num_field("presence_penalty", "Presence penalty", &[], "-2.0 – 2.0", ""),
        &["openai", "monshoot"],
    ));
    fields.push(for_providers(
        sel_field(
            "response_format",
            "Response format",
            &[],
            opts(&[
                ("", "— default —"),
                ("text", "text"),
                ("json_object", "json_object"),
                ("json_schema", "json_schema"),
            ]),
            "structured output",
        ),
        &["openai", "monshoot"],
    ));

    fields.push(for_providers(
        num_field("thinking_budget", "Thinking budget", &[], "tokens", "thinkingConfig budget"),
        &["gemini"],
    ));

    fields.push(for_providers(
        num_field("num_ctx", "Context size", &[], "e.g. 8192", "context window (num_ctx)"),
        &["ollama"],
    ));
    fields.push(for_providers(
        num_field("repeat_penalty", "Repeat penalty", &[], "e.g. 1.1", ""),
        &["ollama"],
    ));
    fields.push(for_providers(num_field("min_p", "Min-p", &[], "0.0 – 1.0", ""), &["ollama"]));
    fields.push(for_providers(
        txt_field("keep_alive", "Keep alive", &[], "5m / -1", "how long to keep the model loaded"),
        &["ollama"],
    ));
    fields.push(for_providers(chk_field("think", "Thinking", &[]), &["ollama"]));
    fields.push(for_providers(
        sel_field(
            "format",
            "Response format",
            &[],
            opts(&[("", "— default —"), ("json", "json")]),
            "structured output",
        ),
        &["ollama"],
    ));

    // ---- harness:adi sampling (provider-scoped) ----
    // temperature is left OFF the providers where a non-default value 400s: Anthropic current
    // models, OpenAI o-series/gpt-5, and Monshoot kimi-k2.6 (verified). It stays only where it's
    // a normal knob — Gemini and Ollama.
    fields.push(for_providers(
        num_field("temperature", "Temperature", &[], "0.0 – 2.0", ""),
        &["gemini", "ollama"],
    ));
    fields.push(for_providers(
        num_field("top_p", "Top-p", &[], "0.0 – 1.0", ""),
        &["openai", "gemini", "monshoot", "ollama"],
    ));
    fields.push(for_providers(
        num_field("top_k", "Top-k", &[], "e.g. 40", ""),
        &["gemini", "ollama"],
    ));
    fields.push(for_providers(
        num_field("seed", "Seed", &[], "e.g. 42", "deterministic sampling"),
        &["openai", "gemini", "ollama"],
    ));

    // ---- harness:adi shared (whatever the provider) ----
    let mut max_tokens =
        field_ids("max_tokens", "Max output tokens", AgentFormFieldKind::Number, &[ADI_HARNESS]);
    max_tokens.placeholder = "e.g. 4096".into();
    max_tokens.hint = "maps to each provider's output-cap field".into();
    max_tokens.numeric = true;
    fields.push(max_tokens);

    let mut stop = field_ids("stop", "Stop sequences", AgentFormFieldKind::Text, &[ADI_HARNESS]);
    stop.placeholder = "comma-separated".into();
    stop.hint = "stop generation on these strings".into();
    stop.mono = true;
    stop.wide = true;
    fields.push(stop);

    let mut max_turns = agent_field("max_turns", "Max turns", AgentFormFieldKind::Number);
    max_turns.placeholder = "optional".into();
    max_turns.hint = "harness cap on agent turns per run".into();
    max_turns.numeric = true;
    fields.push(max_turns);

    let mut api_key_env =
        field_ids("api_key_env", "API key env", AgentFormFieldKind::Text, &[ADI_HARNESS]);
    api_key_env.placeholder = "OPENAI_API_KEY".into();
    api_key_env.hint = "environment variable read for the chosen provider".into();
    api_key_env.mono = true;
    fields.push(api_key_env);

    let mut base_url = field_ids("base_url", "Base URL", AgentFormFieldKind::Text, &[ADI_HARNESS]);
    base_url.placeholder = "provider endpoint override".into();
    base_url.hint = "e.g. https://api.moonshot.ai/v1 · http://localhost:11434".into();
    base_url.mono = true;
    base_url.wide = true;
    fields.push(base_url);

    // ---- always shown ----
    fields.push(agent_field("starred", "Starred", AgentFormFieldKind::Checkbox));

    let mut tags = agent_field("tags", "Tags", AgentFormFieldKind::Text);
    tags.placeholder = "comma-separated (dispatch / filtering)".into();
    tags.wide = true;
    fields.push(tags);

    let mut tools = agent_field("tools", "CLI commands", AgentFormFieldKind::Text);
    tools.placeholder = "tasks,projects,agents".into();
    tools.hint = "which adi-mono command groups this agent may use".into();
    tools.mono = true;
    tools.wide = true;
    fields.push(tools);

    let mut prompt = agent_field(
        "system_prompt",
        "System prompt",
        AgentFormFieldKind::Textarea,
    );
    prompt.placeholder = "The system prompt that seeds this agent...".into();
    prompt.wide = true;
    fields.push(prompt);

    AgentFormSpec {
        backends: vec![
            agent_backend(
                "tmux:claude",
                "tmux · Claude CLI",
                "tmux",
                "opus / sonnet / fable / haiku",
            ),
            agent_backend("tmux:codex", "tmux · Codex CLI", "tmux", "gpt-5-codex"),
            agent_backend(
                "process:claude",
                "process · Claude CLI",
                "process",
                "opus / sonnet / fable / haiku",
            ),
            agent_backend("process:codex", "process · Codex CLI", "process", "gpt-5-codex"),
            agent_backend(
                "harness:claude-sdk",
                "harness · Claude SDK",
                "harness",
                "claude-opus-4-8 / claude-sonnet-5",
            ),
            agent_backend(
                ADI_HARNESS,
                "harness · ADI loop",
                "harness",
                "provider model, e.g. kimi-k2.6 / gemini-2.5-pro",
            ),
        ],
        fields,
    }
}

fn agent_backend(id: &str, label: &str, executor: &str, model_placeholder: &str) -> AgentBackendOption {
    AgentBackendOption {
        id: id.into(),
        label: label.into(),
        executor: executor.into(),
        model_placeholder: model_placeholder.into(),
    }
}

fn agent_field(name: &str, label: &str, kind: AgentFormFieldKind) -> AgentFormField {
    AgentFormField {
        name: name.into(),
        label: label.into(),
        kind,
        placeholder: String::new(),
        hint: String::new(),
        options: Vec::new(),
        backend_ids: Vec::new(),
        executors: Vec::new(),
        providers: Vec::new(),
        mono: false,
        wide: false,
        numeric: false,
        required: false,
    }
}

fn agent_option(value: &str, label: &str) -> AgentFormOption {
    AgentFormOption {
        value: value.into(),
        label: label.into(),
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

/// A field visible only for specific backend ids (e.g. `tmux:claude`).
fn field_ids(name: &str, label: &str, kind: AgentFormFieldKind, ids: &[&str]) -> AgentFormField {
    let mut f = agent_field(name, label, kind);
    f.backend_ids = strings(ids);
    f
}

/// A field visible for whole executors (`tmux` / `process` / `harness`).
fn field_executors(
    name: &str,
    label: &str,
    kind: AgentFormFieldKind,
    executors: &[&str],
) -> AgentFormField {
    let mut f = agent_field(name, label, kind);
    f.executors = strings(executors);
    f
}

/// Also show a field when `harness:adi` targets one of these providers (on top of whatever
/// backend-id scoping the field already carries).
fn for_providers(mut f: AgentFormField, providers: &[&str]) -> AgentFormField {
    f.providers = strings(providers);
    f
}

/// A select field scoped to backend ids, with a hint.
fn sel_field(
    name: &str,
    label: &str,
    ids: &[&str],
    options: Vec<AgentFormOption>,
    hint: &str,
) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Select, ids);
    f.options = options;
    f.hint = hint.into();
    f
}

/// A numeric field scoped to backend ids.
fn num_field(name: &str, label: &str, ids: &[&str], placeholder: &str, hint: &str) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Number, ids);
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.numeric = true;
    f
}

/// A monospace text field scoped to backend ids.
fn txt_field(name: &str, label: &str, ids: &[&str], placeholder: &str, hint: &str) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Text, ids);
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.mono = true;
    f
}

/// A checkbox scoped to backend ids (stored as a `"true"` string in `extra`).
fn chk_field(name: &str, label: &str, ids: &[&str]) -> AgentFormField {
    field_ids(name, label, AgentFormFieldKind::Checkbox, ids)
}

/// Build a select-option list from `(value, label)` pairs.
fn opts(pairs: &[(&str, &str)]) -> Vec<AgentFormOption> {
    pairs.iter().map(|&(v, l)| agent_option(v, l)).collect()
}

/// Map an agent-store error to an HTTP status: bad name / unrunnable backend / bad key → 400,
/// missing → 404, wrong run state (already / not running) → 409, else 500.
fn agent_error(e: &AgentStoreError) -> (u16, String) {
    let status = match e {
        AgentStoreError::InvalidName(_)
        | AgentStoreError::NotRunnable(_)
        | AgentStoreError::InvalidKey(_) => 400,
        AgentStoreError::NotFound(_) => 404,
        AgentStoreError::AlreadyRunning(_) | AgentStoreError::NotRunning(_) => 409,
        AgentStoreError::Config(_)
        | AgentStoreError::Io(_)
        | AgentStoreError::Launch(_)
        | AgentStoreError::Tmux(_) => 500,
    };
    error(status, &e.to_string())
}

fn parse_save_agent(body: &[u8]) -> Option<SaveAgent> {
    let req: SaveAgent = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.backend.trim().is_empty()).then_some(req)
}

fn bad_save_agent() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"backend\": \"…\", … } with a non-empty name and backend",
    )
}

fn parse_agent_ref(body: &[u8]) -> Option<AgentRef> {
    let req: AgentRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn bad_agent_ref() -> (u16, String) {
    error(400, "expected JSON body { \"name\": \"…\" }")
}

fn parse_agent_keys(body: &[u8]) -> Option<AgentKeys> {
    let req: AgentKeys = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && (!req.text.is_empty() || !req.key.is_empty())).then_some(req)
}

fn bad_agent_keys() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"text\": \"…\", \"key\": \"…\" } with a non-empty name and at least one of text/key",
    )
}

/// Trim a string, dropping it entirely when blank (so an empty optional field clears).
fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Trim dynamic backend parameters and drop empty or unsafe keys.
fn clean_extra(extra: BTreeMap<String, String>) -> BTreeMap<String, String> {
    extra
        .into_iter()
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty() && safe_extra_key(k))
        .collect()
}

fn safe_extra_key(key: &str) -> bool {
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
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

// MARK: triggers — background code blocks fired by webhooks & co. (~/.adi/mono/triggers)

/// `GET /api/triggers` — every registered trigger plus the selectable kinds. Each mutation
/// endpoint below returns a fresh [`TriggersState`], so the client refreshes from one round-trip.
#[must_use]
pub fn triggers(store: &Triggers) -> (u16, String) {
    match triggers_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => trigger_error(&e),
    }
}

/// The full [`TriggersState`]: the stored definitions decorated with their last-fired time,
/// plus the server-owned kind options.
fn triggers_state(store: &Triggers) -> Result<TriggersState, TriggerStoreError> {
    Ok(TriggersState {
        triggers: store
            .list()?
            .into_iter()
            .map(|t| trigger_dto(store, t))
            .collect(),
        kinds: trigger_kinds(),
    })
}

/// `POST /api/triggers/save` — create or update a trigger definition (an upsert keyed by
/// `name`), then report the fresh list. `name` and `kind` are required.
#[must_use]
pub fn save_trigger(store: &Triggers, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_save_trigger(body) else {
        return bad_save_trigger();
    };
    let name = req.name.trim().to_string();
    let manifest = TriggerManifest {
        kind: req.kind.trim().to_string(),
        code: req.code,
        description: req.description.trim().to_string(),
        enabled: req.enabled,
        project: clean(req.project),
        extra: clean_extra(req.extra),
        // The store owns the timestamps.
        created_at: 0,
        updated_at: 0,
    };
    match store.save(&name, manifest) {
        Ok(_) => triggers(store),
        Err(e) => trigger_error(&e),
    }
}

/// `POST /api/triggers/delete` — delete a trigger definition, then report the fresh list.
#[must_use]
pub fn delete_trigger(store: &Triggers, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    match store.delete(req.name.trim()) {
        Ok(_) => triggers(store),
        Err(e) => trigger_error(&e),
    }
}

/// `POST /api/triggers/fire` — fire a trigger by hand (no payload). An explicit user action, so
/// it works even on a disabled trigger — only the *external* sources are gated by `enabled`.
/// Replies with the spawned pid plus fresh state.
#[must_use]
pub fn fire_trigger(store: &Triggers, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    let name = req.name.trim();
    let firing = match store.fire(name, None) {
        Ok(firing) => firing,
        Err(e) => return trigger_error(&e),
    };
    match triggers_state(store) {
        Ok(state) => ok_json(&TriggerFireResult {
            message: format!("Fired “{name}” (pid {}).", firing.pid),
            state,
        }),
        Err(e) => trigger_error(&e),
    }
}

/// `POST /api/triggers/log` — the tail of a trigger's most recent fire log. A registered
/// trigger that never fired answers `fired: false` (200, not an error); only an unknown name
/// is a 404.
#[must_use]
pub fn trigger_log(store: &Triggers, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    let name = req.name.trim();
    match store.get(name) {
        Ok(Some(trigger)) => {
            let output = store.read_log(&trigger.name);
            ok_json(&TriggerLog {
                fired: output.is_some(),
                output: output.unwrap_or_default(),
                fired_at: store.last_fired(&trigger.name),
                name: trigger.name,
            })
        }
        Ok(None) => trigger_error(&TriggerStoreError::NotFound(name.to_string())),
        Err(e) => trigger_error(&e),
    }
}

/// `POST|GET /api/hooks/<name>` — the public webhook endpoint: fire the named trigger with the
/// request body as its payload. Only an **enabled** trigger of the `webhook` kind fires; when
/// its `secret` extra is set, the caller must match it with a `?secret=` query parameter.
/// An unknown name and a non-webhook trigger answer the same 404, so the endpoint doesn't
/// reveal which internal names exist.
#[must_use]
pub fn hook_trigger(store: &Triggers, name: &str, query: &str, payload: &[u8]) -> (u16, String) {
    let trigger = match store.get(name) {
        Ok(Some(t)) => t,
        // An unregistered and an unsafely-named hook answer identically, revealing nothing.
        Ok(None) | Err(TriggerStoreError::InvalidName(_)) => {
            return error(404, &format!("no such hook: {name}"));
        }
        Err(e) => return trigger_error(&e),
    };
    if trigger.manifest.kind != adi_triggers::KIND_WEBHOOK {
        return error(404, &format!("no such hook: {name}"));
    }
    if !trigger.manifest.enabled {
        return error(403, &format!("hook {name} is disabled"));
    }
    if let Some(secret) = trigger.manifest.extra.get("secret").filter(|s| !s.is_empty())
        && query_param(query, "secret") != Some(secret.as_str())
    {
        return error(403, "bad or missing secret");
    }
    match store.fire(&trigger.name, Some(payload)) {
        Ok(_) => ok_json(&HookAck {
            ok: true,
            trigger: trigger.name,
        }),
        Err(e) => trigger_error(&e),
    }
}

/// The value of `key` in a raw query string (`a=1&b=2`), undecoded. Webhook secrets are plain
/// tokens (letters/digits/dashes), so URL-decoding is deliberately skipped.
fn query_param<'q>(query: &'q str, key: &str) -> Option<&'q str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// The selectable trigger kinds — server-owned so adding one doesn't require a webapp rebuild.
fn trigger_kinds() -> Vec<TriggerKindOption> {
    let kind = |id: &str, label: &str, hint: &str| TriggerKindOption {
        id: id.into(),
        label: label.into(),
        hint: hint.into(),
    };
    vec![
        kind(
            adi_triggers::KIND_WEBHOOK,
            "Webhook",
            "fires on POST/GET to /api/hooks/<name>; optional shared secret",
        ),
        kind(
            adi_triggers::KIND_TELEGRAM,
            "Telegram",
            "bot-update listener is future work — define now, fire manually to test",
        ),
        kind(
            adi_triggers::KIND_CRON,
            "Cron",
            "scheduler runtime is future work — define now, fire manually to test",
        ),
        kind(adi_triggers::KIND_MANUAL, "Manual", "fired only by hand (UI / CLI / API)"),
    ]
}

/// Flatten a stored trigger into its wire [`TriggerDto`], decorated with its last-fired time.
fn trigger_dto(store: &Triggers, trigger: adi_triggers::Trigger) -> TriggerDto {
    let last_fired_at = store.last_fired(&trigger.name);
    let m = trigger.manifest;
    TriggerDto {
        name: trigger.name,
        kind: m.kind,
        code: m.code,
        description: m.description,
        enabled: m.enabled,
        project: m.project,
        extra: m.extra,
        created_at: m.created_at,
        updated_at: m.updated_at,
        last_fired_at,
    }
}

/// Map a trigger-store error to an HTTP status: bad name / no code → 400, missing → 404, else 500.
fn trigger_error(e: &TriggerStoreError) -> (u16, String) {
    let status = match e {
        TriggerStoreError::InvalidName(_) | TriggerStoreError::NoCode(_) => 400,
        TriggerStoreError::NotFound(_) => 404,
        TriggerStoreError::Config(_) | TriggerStoreError::Io(_) | TriggerStoreError::Launch(_) => {
            500
        }
    };
    error(status, &e.to_string())
}

fn parse_save_trigger(body: &[u8]) -> Option<SaveTrigger> {
    let req: SaveTrigger = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.kind.trim().is_empty()).then_some(req)
}

fn bad_save_trigger() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"kind\": \"…\", … } with a non-empty name and kind",
    )
}

fn parse_trigger_ref(body: &[u8]) -> Option<TriggerRef> {
    let req: TriggerRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn bad_trigger_ref() -> (u16, String) {
    error(400, "expected JSON body { \"name\": \"…\" }")
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

    fn temp_agents() -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-agents-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(adi_config::Config::with_root(root))
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

    #[test]
    fn agents_response_includes_form_schema() {
        let store = temp_agents();
        let (status, body) = agents(&store);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();

        let backends = v["form"]["backends"].as_array().unwrap();
        // Backend ids are executor:what pairs; the executor is the run mechanism.
        assert!(
            backends
                .iter()
                .any(|b| b["id"] == "tmux:claude" && b["executor"] == "tmux")
        );
        assert!(
            backends
                .iter()
                .any(|b| b["id"] == "harness:adi" && b["executor"] == "harness")
        );

        let fields = v["form"]["fields"].as_array().unwrap();
        assert!(fields.iter().any(|f| f["name"] == "api_key_env"));
        // permission_mode is Claude-only (Codex uses sandbox / approval instead).
        assert!(fields.iter().any(|f| {
            f["name"] == "permission_mode"
                && f["backend_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|id| id == "tmux:claude")
        }));
        // Backend/provider-specific params are present.
        for name in ["effort", "sandbox", "approval", "thinking", "num_ctx", "max_tokens"] {
            assert!(fields.iter().any(|f| f["name"] == name), "missing field {name}");
        }
        // Temperature applies only where a non-default value is safe (the Gemini and Ollama
        // providers) — not the reasoning / current-model providers where it 400s.
        let temperature = fields.iter().find(|f| f["name"] == "temperature").unwrap();
        let providers = temperature["providers"].as_array().unwrap();
        assert!(providers.iter().any(|p| p == "ollama"));
        assert!(!providers.iter().any(|p| p == "anthropic"));
    }

    #[test]
    fn agents_report_runnable_for_tmux_backends_only() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"tmux:claude"}"#);
        let _ = save_agent(&store, br#"{"name":"looper","backend":"harness:adi"}"#);

        let (status, body) = agents(&store);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let list = v["agents"].as_array().unwrap();
        let looper = list.iter().find(|a| a["name"] == "looper").unwrap();
        let solver = list.iter().find(|a| a["name"] == "solver").unwrap();
        assert_eq!(looper["runnable"], false);
        assert_eq!(solver["runnable"], true);
        assert_eq!(looper["running"], false);
    }

    #[test]
    fn run_of_a_missing_agent_is_404() {
        let store = temp_agents();
        let (status, _) = run_agent(&store, br#"{"name":"ghost"}"#);
        assert_eq!(status, 404);
    }

    #[test]
    fn peek_reports_not_running_for_a_sessionless_agent() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"tmux:claude"}"#);

        let (status, body) = peek_agent(&store, br#"{"name":"solver"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["running"], false);
        assert_eq!(v["output"], "");
        assert_eq!(v["attach"], "tmux attach -t adi-agent-solver");

        // Only an unknown name is an error.
        assert_eq!(peek_agent(&store, br#"{"name":"ghost"}"#).0, 404);
    }

    #[test]
    fn send_keys_validates_body_and_run_state() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"tmux:claude"}"#);

        // Unknown agent → 404; a body with neither text nor key → 400.
        assert_eq!(send_agent_keys(&store, br#"{"name":"ghost","key":"Enter"}"#).0, 404);
        assert_eq!(send_agent_keys(&store, br#"{"name":"solver"}"#).0, 400);

        // Registered but sessionless → 409 (nothing to type into).
        let (status, body) = send_agent_keys(&store, br#"{"name":"solver","text":"hi"}"#);
        assert_eq!(status, 409);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("isn't running"));
    }

    #[test]
    fn stop_is_idempotent_and_404s_unknown() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"tmux:claude"}"#);

        // Stopping a registered agent with no session succeeds (idempotent no-op) and returns
        // the fresh list; an unknown agent is a 404.
        let (status, body) = stop_agent(&store, br#"{"name":"solver"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["agents"].as_array().unwrap().iter().any(|a| a["name"] == "solver"));
        assert_eq!(stop_agent(&store, br#"{"name":"ghost"}"#).0, 404);
    }

    #[test]
    fn run_of_an_unrunnable_backend_is_400() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"looper","backend":"harness:adi"}"#);
        let (status, body) = run_agent(&store, br#"{"name":"looper"}"#);
        assert_eq!(status, 400);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("can't be run yet"));
    }

    #[test]
    fn save_agent_round_trips_extra_params() {
        let store = temp_agents();
        let (status, body) = save_agent(
            &store,
            br#"{
                "name":"api-solver",
                "backend":"api:openai",
                "extra":{
                    "api_key_env":" OPENAI_API_KEY ",
                    "base_url":" http://localhost:11434 ",
                    "bad key":"drop",
                    "empty":""
                }
            }"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let agent = &v["agents"].as_array().unwrap()[0];
        assert_eq!(agent["extra"]["api_key_env"], "OPENAI_API_KEY");
        assert_eq!(agent["extra"]["base_url"], "http://localhost:11434");
        assert!(agent["extra"]["bad key"].is_null());
        assert!(agent["extra"]["empty"].is_null());
    }

    // ---- triggers ----------------------------------------------------------------------

    fn temp_triggers() -> Triggers {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-triggers-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Triggers::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn triggers_response_includes_the_kind_options() {
        let store = temp_triggers();
        let (status, body) = triggers(&store);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["triggers"].as_array().unwrap().is_empty());
        let kinds: Vec<&str> = v["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["id"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["webhook", "telegram", "cron", "manual"]);
    }

    #[test]
    fn save_trigger_round_trips_and_cleans_extras() {
        let store = temp_triggers();
        let (status, body) = save_trigger(
            &store,
            br#"{
                "name":"deploy-hook",
                "kind":"webhook",
                "code":"echo deployed",
                "description":" redeploy on push ",
                "project":" demo ",
                "extra":{ "secret":" s3cr3t ", "bad key":"drop", "empty":"" }
            }"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let t = &v["triggers"].as_array().unwrap()[0];
        assert_eq!(t["name"], "deploy-hook");
        assert_eq!(t["kind"], "webhook");
        assert_eq!(t["enabled"], true);
        assert_eq!(t["project"], "demo");
        assert_eq!(t["description"], "redeploy on push");
        assert_eq!(t["extra"]["secret"], "s3cr3t");
        assert!(t["extra"]["bad key"].is_null());
        assert!(t["extra"]["empty"].is_null());
        assert!(t["last_fired_at"].is_null());

        // Name and kind are both required.
        assert_eq!(save_trigger(&store, br#"{"name":"x","kind":""}"#).0, 400);
        assert_eq!(save_trigger(&store, b"not json").0, 400);
    }

    #[test]
    fn fire_validates_the_target() {
        let store = temp_triggers();
        // Unknown trigger → 404; a codeless one → 400.
        assert_eq!(fire_trigger(&store, br#"{"name":"ghost"}"#).0, 404);
        let _ = save_trigger(&store, br#"{"name":"idle","kind":"manual"}"#);
        assert_eq!(fire_trigger(&store, br#"{"name":"idle"}"#).0, 400);
    }

    #[test]
    fn log_of_a_never_fired_trigger_is_empty_not_an_error() {
        let store = temp_triggers();
        let _ = save_trigger(&store, br#"{"name":"idle","kind":"manual","code":"true"}"#);
        let (status, body) = trigger_log(&store, br#"{"name":"idle"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["fired"], false);
        assert_eq!(v["output"], "");
        assert_eq!(trigger_log(&store, br#"{"name":"ghost"}"#).0, 404);
    }

    #[test]
    fn hook_gates_on_kind_enabled_and_secret() {
        let store = temp_triggers();
        let _ = save_trigger(
            &store,
            br#"{"name":"manual-only","kind":"manual","code":"true"}"#,
        );
        let _ = save_trigger(
            &store,
            br#"{"name":"paused","kind":"webhook","code":"true","enabled":false}"#,
        );
        let _ = save_trigger(
            &store,
            br#"{"name":"locked","kind":"webhook","code":"true","extra":{"secret":"s3"}}"#,
        );

        // Unknown, unsafe, and non-webhook names all answer the same 404.
        assert_eq!(hook_trigger(&store, "ghost", "", b"").0, 404);
        assert_eq!(hook_trigger(&store, "../etc", "", b"").0, 404);
        assert_eq!(hook_trigger(&store, "manual-only", "", b"").0, 404);
        // Disabled and bad-secret hooks are refused.
        assert_eq!(hook_trigger(&store, "paused", "", b"").0, 403);
        assert_eq!(hook_trigger(&store, "locked", "", b"").0, 403);
        assert_eq!(hook_trigger(&store, "locked", "secret=wrong", b"").0, 403);
        // The right secret fires it and acks.
        let (status, body) = hook_trigger(&store, "locked", "x=1&secret=s3", b"{\"ref\":\"main\"}");
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["trigger"], "locked");
    }

    // ---- files -----------------------------------------------------------------------

    /// A projects store rooted in an isolated temp dir, with a registered `demo` project whose
    /// `.adi/hive.yaml` exists (mirroring the real on-disk layout).
    fn temp_projects() -> Projects {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-files-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = Projects::with_config(adi_config::Config::with_root(&root));
        store.create("demo", Some("Demo".into()), None, None).unwrap();
        let hive = store.hive_path("demo").unwrap();
        std::fs::create_dir_all(hive.parent().unwrap()).unwrap();
        std::fs::write(&hive, b"version: \"1\"\n").unwrap();
        store
    }

    #[test]
    fn list_files_shows_the_project_tree() {
        let store = temp_projects();
        let (status, body) = list_files(&store, br#"{"id":"demo","path":""}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["path"], "");
        assert!(v["parent"].is_null());
        let names: Vec<&str> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&".adi"));
        assert!(names.contains(&"config.toml"));

        // Descend into `.adi`; its parent is the root.
        let (_, body) = list_files(&store, br#"{"id":"demo","path":".adi"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["path"], ".adi");
        assert_eq!(v["parent"], "");
    }

    #[test]
    fn read_then_write_round_trips_the_hive_file() {
        let store = temp_projects();
        let (status, body) = read_file(&store, br#"{"id":"demo","path":".adi/hive.yaml"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["content"], "version: \"1\"\n");

        let (status, body) = write_file(
            &store,
            br#"{"id":"demo","path":".adi/hive.yaml","content":"version: \"2\"\n"}"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["content"], "version: \"2\"\n");

        // The write actually hit disk.
        let (_, body) = read_file(&store, br#"{"id":"demo","path":".adi/hive.yaml"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["content"], "version: \"2\"\n");
    }

    #[test]
    fn escaping_paths_are_refused_with_400() {
        let store = temp_projects();
        assert_eq!(list_files(&store, br#"{"id":"demo","path":".."}"#).0, 400);
        assert_eq!(
            read_file(&store, br#"{"id":"demo","path":"../../secret"}"#).0,
            400
        );
        assert_eq!(
            write_file(&store, br#"{"id":"demo","path":"../evil","content":"x"}"#).0,
            400
        );
    }

    #[test]
    fn unregistered_project_is_a_404() {
        let store = temp_projects();
        assert_eq!(list_files(&store, br#"{"id":"ghost","path":""}"#).0, 404);
        // An unsafe id is rejected before any disk access, as a 400.
        assert_eq!(list_files(&store, br#"{"id":"../x","path":""}"#).0, 400);
    }

    #[test]
    fn reading_a_missing_file_is_a_404() {
        let store = temp_projects();
        assert_eq!(
            read_file(&store, br#"{"id":"demo","path":"nope.txt"}"#).0,
            404
        );
    }
}
