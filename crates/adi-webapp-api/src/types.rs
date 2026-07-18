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
    /// The id of the project this one nests under (a sub-project), or `None` for top-level.
    #[serde(default)]
    pub parent: Option<String>,
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

/// Request body registering a project — `POST /api/projects/create`. The server generates the
/// project id (a UUID); callers supply only the display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewProject {
    /// The human-facing display name (required, non-blank).
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The id of the project to nest the new one under (a sub-project); blank/omitted
    /// registers a top-level project. Must name a registered project.
    #[serde(default)]
    pub parent: Option<String>,
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

/// Request body for `POST /api/hive/create` — add a service to a project's `.adi/hive.yaml`.
/// Responds with the fresh [`ProjectDetail`] so the page updates in one round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewService {
    /// The owning project id (services are always project-scoped; the global front-door
    /// hive is hand-edited, not API-managed).
    pub project: String,
    /// The service name — the key under `services:` and the ports-manager lease segment.
    pub name: String,
    /// The runner command (`runner.script.run`), executed via `sh -c`.
    pub run: String,
    /// The proxied host (`proxy.host`, e.g. `demo.adi`); omitted → no front-door route.
    #[serde(default)]
    pub host: Option<String>,
    /// An explicit `http` port; omitted → a `` ports-manager.get('<project>/<name>', 'http') ``
    /// command is written instead, so the port is leased on read.
    #[serde(default)]
    pub port: Option<u16>,
    /// The runner's working directory, relative to the project dir (`runner.script.working_dir`).
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Restart policy (`always` | `on-failure` | `no`); omitted → adi-hive's default.
    #[serde(default)]
    pub restart: Option<String>,
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
    /// The id of the project this one nests under (a sub-project), or `None` for top-level.
    #[serde(default)]
    pub parent: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub archived_at: Option<u64>,
    /// Whether a `.adi/hive.yaml` exists for this project.
    pub has_hive: bool,
    pub services: Vec<ProjectService>,
    /// The direct sub-projects of this project, sorted by id — so the detail page lists them
    /// without a second request.
    #[serde(default)]
    pub subprojects: Vec<Project>,
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
/// run mechanism (`tmux` / `process` / `harness` / `wasm`) and the suffix is what it runs.
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
/// `providers` matches the `provider` argument of the `harness:adi` backend only.
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

/// One agent definition on the wire. ADI-owned metadata remains top-level; everything interpreted
/// by the selected backend is nested under `arguments`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDto {
    pub name: String,
    pub backend: String,
    #[serde(default)]
    pub arguments: BTreeMap<String, serde_json::Value>,
    pub executor: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub starred: bool,
    /// The project this agent is filed under (its id), or `None` for a global agent.
    #[serde(default)]
    pub project: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Whether this agent's backend has a run adapter, i.e. whether ▶ Run can work at all.
    #[serde(default)]
    pub runnable: bool,
    /// Whether this agent has a live tmux session or detached process right now.
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
/// keyed by `name`). `name` and `backend` are required; backend settings live in `arguments`.
/// Timestamps are owned by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveAgent {
    pub name: String,
    pub backend: String,
    #[serde(default)]
    pub arguments: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub starred: bool,
    /// The project to file the agent under (its id); blank/omitted saves a global agent.
    #[serde(default)]
    pub project: Option<String>,
}

/// Request body naming an agent — `POST /api/agents/delete` and `POST /api/agents/run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRef {
    pub name: String,
}

/// `POST /api/agents/run` — a human-readable launch outcome plus fresh agent state.
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

/// Request body writing a wasm agent's employee source — `POST /api/agents/code/save`. The
/// target file is the agent's `src` argument; replies with the fresh [`AgentCode`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveAgentCode {
    pub name: String,
    #[serde(default)]
    pub code: String,
}

/// A wasm agent's employee source file — the answer to `POST /api/agents/code` and
/// `/api/agents/code/save`. `path` is the manifest's `src` argument, resolved server-side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCode {
    pub name: String,
    pub path: String,
    pub code: String,
}

/// The answer to `POST /api/agents/build` — the TS→WASM build's combined output plus the fresh
/// agents state (a successful build fills in an empty `wasm` argument, changing the list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentBuildResult {
    pub ok: bool,
    pub output: String,
    /// The compiled component path the build targets (`<src dir>/build/<name>.wasm`).
    pub wasm: String,
    pub state: AgentsState,
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

// ---- triggers (background code blocks fired by webhooks & co., under ~/.adi/mono/triggers) ----

/// One selectable trigger kind: its id (`webhook` / `telegram` / `cron` / `manual`), a display
/// label, and a hint about how (or whether, yet) that source fires. Server-owned so adding a
/// kind doesn't require a webapp rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerKindOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
}

/// One trigger definition, flattened for the wire. `kind` names the event source; `code` is the
/// shell block spawned detached on fire; `last_fired_at` is derived from the fire log's mtime
/// (`None` if it never fired).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerDto {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    /// The project this trigger is filed under (its id), or `None` for a global trigger.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub last_fired_at: Option<u64>,
}

/// `GET /api/triggers` — every registered trigger, sorted by name, plus the selectable kinds.
/// Each mutation endpoint returns a fresh one, so the client refreshes from one round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggersState {
    pub triggers: Vec<TriggerDto>,
    pub kinds: Vec<TriggerKindOption>,
}

/// Request body for `POST /api/triggers/save` — create or update a trigger definition (an
/// upsert keyed by `name`). `name` and `kind` are required. Timestamps are owned by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTrigger {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "trigger_enabled_default")]
    pub enabled: bool,
    /// The project to file the trigger under (its id); blank/omitted saves a global trigger.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

/// serde default for [`SaveTrigger::enabled`] — an omitted flag saves an enabled trigger.
fn trigger_enabled_default() -> bool {
    true
}

/// Request body naming a trigger — `POST /api/triggers/delete`, `/fire`, and `/log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerRef {
    pub name: String,
}

/// `POST /api/triggers/fire` — the manual-fire outcome: a human-readable message (the spawned
/// pid), plus the fresh triggers state so the client refreshes in the same round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerFireResult {
    pub message: String,
    pub state: TriggersState,
}

/// `POST /api/triggers/log` — the tail of a trigger's most recent fire log. `fired` is false
/// (with an empty `output`) when the trigger never fired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerLog {
    pub name: String,
    pub fired: bool,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub fired_at: Option<u64>,
}

/// The response an external webhook caller gets from `/api/hooks/<name>`: an acknowledgement
/// that the named trigger's code block was spawned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookAck {
    pub ok: bool,
    pub trigger: String,
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

// ---- project workspaces & hooks (script files under <project>/.adi/hooks + the
// ---- .adi/workspaces.toml registry) ---------------------------------------------------

/// One project hook file (`.adi/hooks/<name>`) decorated with its last-run status, which is
/// derived from the exit marker its runner appends to `.adi/hooks/logs/<name>.log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHookDto {
    /// The hook's name — its file name under `.adi/hooks/` (also its editable path in the
    /// project file browser).
    pub name: String,
    /// The script's size in bytes.
    pub size: u64,
    /// The script's mtime as Unix epoch seconds.
    #[serde(default)]
    pub modified: Option<u64>,
    /// The most recent run: `never` | `running` | `ok` | `failed`.
    pub status: String,
    /// The finished run's exit code (`0` for `ok`), or `None` while running / never ran.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// When the hook last ran, as Unix epoch seconds (the log's mtime).
    #[serde(default)]
    pub last_run_at: Option<u64>,
}

/// One registered workspace (a working copy the project owns) with its live status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDto {
    pub name: String,
    /// The workspace's absolute directory.
    pub path: String,
    /// How it came to be: `init` | `workspace` (hook-created) | `local` (linked as-is).
    pub kind: String,
    /// Live status: `local` | `creating` (hook run alive) | `ready` | `failed`.
    pub status: String,
    /// The creating hook run's pid (`None` for local links).
    #[serde(default)]
    pub pid: Option<u32>,
    /// The hook that created it (`None` for local links).
    #[serde(default)]
    pub hook: Option<String>,
    pub created_at: u64,
    /// Whether this is the primary workspace — the first hook-created one, which later
    /// `workspace`-hook runs use as their working directory.
    #[serde(default)]
    pub primary: bool,
}

/// `POST /api/projects/workspaces` — a project's workspaces and hooks in one snapshot. Every
/// mutation endpoint in this family returns a fresh one, so the client refreshes from one
/// round-trip. `next_hook` names the lifecycle hook the next hook-backed create would run
/// (`init` while none exists, `workspace` afterwards).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacesState {
    pub id: String,
    pub workspaces: Vec<WorkspaceDto>,
    pub hooks: Vec<ProjectHookDto>,
    pub next_hook: String,
    #[serde(default)]
    pub has_init_hook: bool,
    #[serde(default)]
    pub has_workspace_hook: bool,
}

/// Request body naming a project — `POST /api/projects/workspaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacesRef {
    pub id: String,
}

/// Request body for `POST /api/projects/workspaces/create`. Without `path` the workspace is
/// created at `<project>/workspaces/<name>`; with `local` the (absolute) path is linked
/// as-is and no hook runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewWorkspace {
    pub id: String,
    pub name: String,
    /// An absolute directory to use instead of the default location.
    #[serde(default)]
    pub path: Option<String>,
    /// Link an existing directory as-is — run no hook.
    #[serde(default)]
    pub local: bool,
}

/// Request body naming a workspace — `POST /api/projects/workspaces/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub id: String,
    pub name: String,
}

/// Request body naming a project hook — `POST /api/projects/hook/run` and `/hook/log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHookRef {
    pub id: String,
    pub name: String,
}

/// Request body for `POST /api/projects/hook/create` — materialize a hook file from a
/// template (`init` | `workspace` | `blank`, the default). Refused when the file exists;
/// edits go through the project file browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewProjectHook {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub template: Option<String>,
}

/// `POST /api/projects/workspaces/create` — a human-readable message (which hook ran, its
/// pid) plus the fresh state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCreateResult {
    pub message: String,
    pub state: WorkspacesState,
}

/// `POST /api/projects/hook/run` — the manual-run outcome: the spawned pid plus fresh state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHookRunResult {
    pub message: String,
    pub state: WorkspacesState,
}

/// Request body naming a workspace terminal — `POST /api/projects/workspaces/terminal/open`,
/// `/peek`, and `/kill`. `name` is the workspace name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTermRef {
    pub id: String,
    pub name: String,
}

/// Request body for `POST /api/projects/workspaces/terminal/send` — type `text` literally
/// into the terminal, then press `key` (a tmux key name). Either part may be empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTermKeys {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub key: String,
}

/// A workspace terminal snapshot: whether its tmux session is live, the visible pane text,
/// and the takeover command — the workspace twin of `AgentPeek`, polled by the live view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTerm {
    pub id: String,
    pub name: String,
    /// Whether the terminal's tmux session is live; `output` is empty when it isn't.
    pub running: bool,
    /// The visible pane text (trailing whitespace trimmed).
    #[serde(default)]
    pub output: String,
    /// The command a human runs to take the session over: `tmux attach -t adi-ws-…`.
    #[serde(default)]
    pub attach: String,
}

/// `POST /api/projects/hook/log` — the tail of a hook's most recent run log. `ran` is false
/// (with an empty `output`) when the hook never ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHookLog {
    pub id: String,
    pub name: String,
    pub ran: bool,
    #[serde(default)]
    pub output: String,
    /// The most recent run: `never` | `running` | `ok` | `failed`.
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub ran_at: Option<u64>,
}

// ---- hive (every service across all projects + the global front-door) ----------------

/// One service in the aggregated Hive view: where it's declared, its config, and whether it's
/// currently up. Collected from each project's `.adi/hive.yaml` and the global front-door hive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiveService {
    /// The project id this service belongs to, or `None` when it comes from the global
    /// `~/.adi/mono/hive/hive.yaml` or from a dashboard (see `dashboard`).
    #[serde(default)]
    pub project: Option<String>,
    /// The dashboard id this service belongs to, for services supervised out of
    /// `~/.adi/mono/dashboards/<id>/.adi/hive.yaml`. Mutually exclusive with `project`; both
    /// `None` means the front-door hive.
    #[serde(default)]
    pub dashboard: Option<String>,
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

/// One dashboard under `~/.adi/mono/dashboards/<id>/` — a bun-served frontend + backend pair
/// whose UI is authored as loose `.ts` files by agents.
///
/// Deliberately hostname-free: both services are reached on `127.0.0.1:<port>`, so a dashboard
/// depends on nothing but its own supervisor — not on the root front door or DNS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dashboard {
    /// The directory name, which is also how its hive services are keyed (`<id>/frontend`).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Ports leased from the ports manager; `None` until the supervisor has allocated them.
    #[serde(default)]
    pub frontend_port: Option<u16>,
    #[serde(default)]
    pub backend_port: Option<u16>,
    pub frontend_running: bool,
    pub backend_running: bool,
    /// Agent-authored UI panels (`frontend/modules/*.ts`), by module id.
    pub modules: Vec<String>,
    /// Agent-authored endpoints (`backend/routes/*.ts`), by route id.
    pub routes: Vec<String>,
}

/// `POST /api/dashboards/create` — scaffold a new dashboard. The id is generated, so a name is
/// all a new dashboard needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDashboard {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// `GET /api/dashboards` — every dashboard, each with live port and running state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardsState {
    pub dashboards: Vec<Dashboard>,
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
