//! The wire contract shared by the adi webapp (wasm client) and adi-app (server):
//! one plain serde struct per JSON payload. No I/O and no platform dependencies, so this
//! module compiles unchanged for `wasm32-unknown-unknown` — the frontend deserializes the
//! very types the backend serializes.

use serde::{Deserialize, Serialize};

/// `GET /api/health` — liveness plus identity and uptime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub service: String,
    pub version: String,
    pub uptime_secs: u64,
}

/// An inclusive `[start, end]` port interval — used for both the allocatable range and
/// each reserved band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: u16,
    pub end: u16,
}

/// One static port lease: a `(service, key)` pair bound to a port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub service: String,
    pub key: String,
    pub port: u16,
}

/// `GET /api/ports` — the allocator's configuration and current static leases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortsState {
    pub range: Range,
    pub reserved: Vec<Range>,
    pub leases: Vec<Lease>,
}

/// One TCP port observed in the `LISTEN` state on the machine, with the owning process
/// where the OS reports it. Whether it's ADI-managed is decided by the client, which joins
/// these against the registry [`Lease`]s by port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsedPort {
    pub port: u16,
    pub process: Option<String>,
    pub pid: Option<u32>,
}

/// `GET /api/ports/used` — every listening TCP port on the machine, sorted by port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsedPorts {
    pub ports: Vec<UsedPort>,
}

/// Request body for reserve/release: which `(service, key)` lease to act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRef {
    pub service: String,
    pub key: String,
}

/// `POST /api/ports/reserve` response — the port now held by the pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReserveResponse {
    pub service: String,
    pub key: String,
    pub port: u16,
}

/// `POST /api/ports/release` response — the freed port, or `None` if nothing was held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseResponse {
    pub service: String,
    pub key: String,
    pub freed: Option<u16>,
}

// ---- mesh (peer-to-peer port forwarding over iroh) ---------------------------------

/// `GET /api/mesh` — this machine's mesh identity and config. Every mutation endpoint
/// returns a fresh one of these, so the client updates without a second request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshState {
    /// This machine's `EndpointId` (hex) — the minimal token a peer can dial (via discovery).
    pub id: String,
    /// A ready-to-share ticket (id + relay + direct addresses) the running daemon published,
    /// or `None` when the daemon isn't running.
    pub ticket: Option<String>,
    /// Whether the mesh daemon appears to be running (it publishes a ticket while up).
    pub running: bool,
    /// Local TCP ports this machine exposes to peers.
    pub allow: Vec<u16>,
    /// `EndpointId`s permitted to reach the exposed ports; empty means any peer may.
    pub authorized_peers: Vec<String>,
    /// Local ports this machine forwards to a peer's port.
    pub forwards: Vec<MeshForward>,
}

/// One forward in [`MeshState`]: local `127.0.0.1:listen` tunnels to `peer`'s `port`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshForward {
    pub name: String,
    pub listen: u16,
    pub peer: String,
    pub port: u16,
}

/// Request body naming a port — `POST /api/mesh/allow` and `/api/mesh/deny`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshPortRef {
    pub port: u16,
}

/// Request body naming a peer — `POST /api/mesh/peers/allow` and `/api/mesh/peers/deny`.
/// For `allow` this may be a ticket or an id; the server stores the canonical id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshPeerRef {
    pub peer: String,
}

/// Request body adding a forward — `POST /api/mesh/forwards/add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshForwardRef {
    /// Local TCP port to bind on this machine.
    pub listen: u16,
    /// The peer's ticket or bare `EndpointId`.
    pub peer: String,
    /// The port to reach on the peer.
    pub port: u16,
    /// Optional label; the server derives one from the peer id + port when omitted.
    #[serde(default)]
    pub name: Option<String>,
}

/// Request body removing a forward by its local port — `POST /api/mesh/forwards/remove`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshListenRef {
    pub listen: u16,
}

// ---- projects (metadata manifests under ~/.adi/mono/projects) -----------------------

/// One registered project, flattened for the wire: the id (its directory name) plus the
/// `config.toml` manifest's fields. `archived_at` is `None` while the project is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub archived_at: Option<u64>,
}

impl Project {
    /// Whether the project is archived (soft-deleted).
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

/// `GET /api/projects` — every registered project. Each mutation endpoint returns a fresh
/// one of these, so the client updates without a second request (as the mesh endpoints do).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectsState {
    pub projects: Vec<Project>,
}

/// Request body registering a project — `POST /api/projects/create`. `name` defaults to the
/// id server-side when omitted or blank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewProject {
    /// The project id — its directory name (letters, digits, '.', '-', '_').
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Request body naming a project — `POST /api/projects/archive`, `/unarchive`, and `/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRef {
    pub id: String,
}

/// Request body for `POST /api/hive/start` — launch one hive service's runner. `project` is the
/// owning project id, or `None` for the global front-door hive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartService {
    #[serde(default)]
    pub project: Option<String>,
    pub service: String,
}

/// Response from `POST /api/hive/start` — the launched service, its injected port, and the child pid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartResult {
    pub service: String,
    pub port: Option<u16>,
    pub pid: u32,
}

/// One named port a service declares (`rollout.recreate.ports.<key> = <port>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServicePort {
    pub key: String,
    pub port: u16,
}

/// A service read from a project's `.adi/hive.yaml` — a read-only summary for the detail view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectService {
    pub name: String,
    /// The proxied host (`proxy.host`), e.g. `demo.adi`.
    #[serde(default)]
    pub host: Option<String>,
    /// Declared ports (`rollout.recreate.ports`).
    #[serde(default)]
    pub ports: Vec<ServicePort>,
    /// The runner command (`runner.script.run`), if the service runs a local process.
    #[serde(default)]
    pub run: Option<String>,
    /// Restart policy (`restart`), e.g. `on-failure`.
    #[serde(default)]
    pub restart: Option<String>,
}

/// `GET /api/projects/<id>` — one project's manifest plus the services parsed from its
/// `.adi/hive.yaml` ("inside" the project). `has_hive` distinguishes "no hive.yaml" from
/// "hive.yaml with no services".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDetail {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub archived_at: Option<u64>,
    /// Whether a `.adi/hive.yaml` exists for this project.
    pub has_hive: bool,
    pub services: Vec<ProjectService>,
}

impl ProjectDetail {
    /// Whether the project is archived (soft-deleted).
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

// ---- files (a project's own directory, browsed through an isolated jail) --------------

/// One entry in a project directory [`DirListing`]: a file or subdirectory with lightweight
/// stats. `is_dir` follows a symlink (it describes the target); `is_symlink` flags a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// The entry's name — a single path segment (join it onto the listing's `path`).
    pub name: String,
    /// Whether the entry is (or points at) a directory.
    pub is_dir: bool,
    /// Whether the entry itself is a symbolic link.
    #[serde(default)]
    pub is_symlink: bool,
    /// The file size in bytes (0 for directories).
    pub size: u64,
    /// Last-modified time as Unix epoch seconds, when the platform reports it.
    #[serde(default)]
    pub modified: Option<u64>,
}

/// Request body for browsing/reading within a project's directory — `POST /api/projects/files`
/// and `/api/projects/file/read`. `path` is relative to the project root (`""` is the root);
/// it may never climb out of it (`..`, absolute paths, and symlink escapes are refused).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesRef {
    /// The project id (its directory under `~/.adi/mono/projects`).
    pub id: String,
    /// The path within the project, relative to its root.
    #[serde(default)]
    pub path: String,
}

/// `POST /api/projects/files` — a directory listing within a project's own directory, browsed
/// through the isolated [`adi_fs`] jail so nothing outside the project is reachable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirListing {
    /// The project id this listing belongs to.
    pub id: String,
    /// The listed directory, relative to the project root (`""` is the root).
    pub path: String,
    /// The parent directory (relative to the project root), or `None` at the root — so the UI
    /// can offer an "up" control without re-deriving it.
    #[serde(default)]
    pub parent: Option<String>,
    /// The directory's entries, sorted directories-first then case-insensitively by name.
    pub entries: Vec<FileEntry>,
}

/// `POST /api/projects/file/read` — one text file's contents, read through the project jail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    /// The project id the file belongs to.
    pub id: String,
    /// The file path, relative to the project root.
    pub path: String,
    /// The file's UTF-8 text (binary files are rejected rather than returned here).
    pub content: String,
    /// The file size in bytes.
    pub size: u64,
    /// Last-modified time as Unix epoch seconds, when the platform reports it.
    #[serde(default)]
    pub modified: Option<u64>,
}

/// Request body for saving a file — `POST /api/projects/file/write`. Writes are atomic and
/// create any missing parent directories within the project. Same jail rules as [`FilesRef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFile {
    /// The project id the file belongs to.
    pub id: String,
    /// The file path to write, relative to the project root.
    pub path: String,
    /// The new UTF-8 text contents.
    pub content: String,
}

// ---- hive (every service across all projects + the global front-door) ----------------

/// One service in the aggregated Hive view: where it's declared, its config, and whether it's
/// currently up. Collected from each project's `.adi/hive.yaml` and the global front-door hive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiveService {
    /// The project id this service belongs to, or `None` for the global `~/.adi/mono/hive/hive.yaml`.
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    #[serde(default)]
    pub host: Option<String>,
    pub ports: Vec<ServicePort>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub restart: Option<String>,
    /// The port `running` was decided on (the `http` port, else the sole declared port).
    #[serde(default)]
    pub primary_port: Option<u16>,
    /// Whether `primary_port` is currently listening on the machine.
    pub running: bool,
}

/// `GET /api/hive` — every Hive service across all projects plus the global front-door hive,
/// each with a live running/stopped flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiveState {
    pub services: Vec<HiveService>,
}

/// A JSON error body: `{ "ok": false, "error": "…" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub ok: bool,
    pub error: String,
}

impl ApiError {
    /// A failed-response body carrying `message` (with `ok` fixed to `false`).
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: message.into(),
        }
    }
}
