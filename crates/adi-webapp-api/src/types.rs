//! The wire contract shared by the adi webapp (wasm client) and adi-app (server):
//! one plain serde struct per JSON payload. No I/O and no platform dependencies, so this
//! module compiles unchanged for `wasm32-unknown-unknown` — the frontend deserializes the
//! very types the backend serializes.

use std::collections::BTreeMap;

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

/// Response from `POST /api/hive/stop` — the stopped service and the port whose listener was killed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopResult {
    pub service: String,
    pub port: Option<u16>,
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
    /// Whether the service's primary port is currently listening.
    #[serde(default)]
    pub running: bool,
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

// ---- tasks (the task tree under ~/.adi/mono/tasks/tasks.json) ------------------------

/// One task, flattened for the wire. `status` is the stored lifecycle state
/// (`open`/`done`/`archived`); `effective` is the computed status
/// (`ready`/`blocked`/`done`/`archived`, derived from the stored state plus direct children).
/// `parent` is the id of the parent task, if any — the client rebuilds the tree from these links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub details: Option<String>,
    pub status: String,
    pub effective: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub children_total: usize,
    pub children_open: usize,
    pub created_at: u64,
    pub updated_at: u64,
}

/// `GET /api/tasks` — every task in the tree as a flat list, ordered by task number. The client
/// nests them into a tree by `parent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasksState {
    pub tasks: Vec<TaskRow>,
}

/// Request body creating a task — `POST /api/tasks/create`. Only `title` is required; a given
/// `parent` must be an existing task id (which makes the new task a subtask). The create endpoint
/// returns a fresh [`TasksState`], so the client refreshes the tree from one round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    #[serde(default)]
    pub details: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
}

// ---- agents (AgentDef definitions under ~/.adi/mono/agents) --------------------------

/// UI/schema metadata for the Agents create/edit form. The backend owns this so adding a
/// backend or exposing another backend-specific parameter doesn't require a webapp rebuild that
/// hardcodes the new option list or placeholder text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFormSpec {
    pub backends: Vec<AgentBackendOption>,
    pub fields: Vec<AgentFormField>,
}

/// One selectable agent backend in the form: an `executor:what` pair, where the executor is the
/// run mechanism (`tmux` / `process` / `harness`) and the suffix is what it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBackendOption {
    pub id: String,
    pub label: String,
    pub executor: String,
    #[serde(default)]
    pub model_placeholder: String,
}

/// One rendered form control. `backend_ids`, `executors`, and `providers` are visibility
/// filters (any match shows the field); all empty means the field is always visible.
/// `providers` matches the `provider` extra of the `harness:adi` backend only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFormField {
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: AgentFormFieldKind,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub options: Vec<AgentFormOption>,
    #[serde(default)]
    pub backend_ids: Vec<String>,
    #[serde(default)]
    pub executors: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub mono: bool,
    #[serde(default)]
    pub wide: bool,
    #[serde(default)]
    pub numeric: bool,
    #[serde(default)]
    pub required: bool,
}

/// A select option for a form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFormOption {
    pub value: String,
    pub label: String,
}

/// The small set of controls the client knows how to render from [`AgentFormField`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFormFieldKind {
    Text,
    Number,
    Select,
    Checkbox,
    Textarea,
}

/// One agent definition, flattened for the wire. `backend` is an `executor:what` string
/// (`tmux:claude`, `process:codex`, `harness:adi`, …); `executor` is the `tmux`/`process`/
/// `harness` prefix, which decides how the agent runs and which params apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDto {
    pub name: String,
    pub backend: String,
    pub executor: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub tools: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Whether this agent's backend has a run adapter (today: tmux executors only), i.e. whether
    /// the ▶ Run action can work at all.
    #[serde(default)]
    pub runnable: bool,
    /// Whether a live `adi-agent-<name>` tmux session for this agent exists right now.
    #[serde(default)]
    pub running: bool,
}

/// `GET /api/agents` — every registered agent definition, sorted by name. Each mutation endpoint
/// returns a fresh one, so the client refreshes from one round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentsState {
    pub agents: Vec<AgentDto>,
    pub form: AgentFormSpec,
}

/// Request body for `POST /api/agents/save` — create or update an agent definition (an upsert
/// keyed by `name`). `name` and `backend` are required; the rest are optional settings, some
/// of which only apply to some backends. Timestamps are owned by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveAgent {
    pub name: String,
    pub backend: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub tools: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

/// Request body naming an agent — `POST /api/agents/delete` and `POST /api/agents/run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRef {
    pub name: String,
}

/// `POST /api/agents/run` — the launch outcome: a human-readable message carrying the tmux
/// attach hint (the server owns the session-naming scheme, so the hint is composed here), plus
/// the fresh agents state so the client refreshes in the same round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub message: String,
    pub state: AgentsState,
}

/// Request body for `POST /api/agents/send-keys` — type into a running agent's tmux session:
/// `text` is sent literally, then `key` (a tmux key name: `Enter`, `Escape`, `Up`, `C-c`, …)
/// is pressed. At least one of the two must be non-empty. Replies with a fresh [`AgentPeek`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKeys {
    pub name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub key: String,
}

/// `POST /api/agents/peek` — a read-only snapshot of a running agent's tmux pane (the text
/// `tmux attach` would show), polled by the Agents page's live view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPeek {
    pub name: String,
    /// Whether the agent's tmux session is live; `output` is empty when it isn't.
    pub running: bool,
    /// The visible pane text (trailing whitespace trimmed).
    #[serde(default)]
    pub output: String,
    /// The command a human runs to take the session over: `tmux attach -t adi-agent-<name>`.
    #[serde(default)]
    pub attach: String,
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
