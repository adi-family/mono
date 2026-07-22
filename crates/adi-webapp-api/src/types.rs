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

/// Request body naming one task — `POST /api/tasks/archive` and `POST /api/tasks/reopen`. Both
/// return a fresh [`TasksState`], so the client refreshes the tree from one round-trip. `cascade`
/// applies only to archive: when set, the task's open descendants are archived along with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRef {
    pub id: String,
    #[serde(default)]
    pub cascade: bool,
}

// ---- tools (user CLIs under ~/.adi/mono/tools, run by agents) ------------------------

/// One registered tool, flattened for the wire. A tool is a small CLI an agent runs. It is
/// either **owned** (its script lives in the store) or **linked** (`path` points at an existing
/// file). `bin_name` is the `.bin/<name>` shim an agent invokes it by. `archived_at` is `None`
/// while the tool is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The script language/interpreter: `sh` or `ts`.
    pub runtime: String,
    /// Whether this tool links an existing file on disk (rather than owning a script in the store).
    #[serde(default)]
    pub linked: bool,
    /// The linked target's absolute path, or `None` for an owned tool.
    #[serde(default)]
    pub path: Option<String>,
    /// The `.bin/<name>` shim file name an agent runs this tool by.
    pub bin_name: String,
    /// The project this tool is filed under (its id), or `None` for a global tool.
    #[serde(default)]
    pub project: Option<String>,
    /// Whether this is a built-in **system** tool (an adi-ecosystem CLI). System tools are their
    /// own category, protected from hard delete, and enabled per-agent like any other tool.
    #[serde(default)]
    pub system: bool,
    pub created_at: u64,
    #[serde(default)]
    pub archived_at: Option<u64>,
}

impl ToolDto {
    /// Whether the tool is archived (soft-deleted).
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

/// `GET /api/tools` — every registered tool, plus the `.bin` directory agents put on their PATH.
/// Each mutation endpoint returns a fresh one, so the client refreshes from one round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsState {
    pub tools: Vec<ToolDto>,
    /// The absolute path of `~/.adi/mono/tools/.bin` — the directory holding the shims.
    pub bin_dir: String,
}

/// Request body creating an **owned** tool — `POST /api/tools/create`. The server generates the
/// id and writes a starter script (unless `content` seeds it). `name` and `runtime` are required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTool {
    pub name: String,
    /// The script language: `sh` or `ts`.
    pub runtime: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The project to file the tool under (its id); blank/omitted saves a global tool.
    #[serde(default)]
    pub project: Option<String>,
    /// Seed the new script with this text instead of the runtime template.
    #[serde(default)]
    pub content: Option<String>,
}

/// Request body linking an existing file as a tool — `POST /api/tools/link`. `name` defaults to
/// the file's stem and `runtime` is inferred from the extension when omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTool {
    /// The absolute or relative path to an existing sh/ts file (never copied).
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

/// Request body naming a tool — `POST /api/tools/archive`, `/unarchive`, `/remove`,
/// `/script/read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRef {
    pub id: String,
}

/// `POST /api/tools/script/read` and `/script/write` — a tool's script text. `path` is the
/// resolved on-disk location (the owned file in the store, or the linked target).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolScript {
    pub id: String,
    pub path: String,
    pub content: String,
    pub runtime: String,
}

/// Request body saving a tool's script — `POST /api/tools/script/write`. Owned scripts are
/// written into the store; a linked tool's target file is written through (the user linked it to
/// edit it here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteToolScript {
    pub id: String,
    pub content: String,
}

/// Request body running a tool — `POST /api/tools/run`. `args` are forwarded to the script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTool {
    pub id: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// `POST /api/tools/run` — the captured outcome of a one-off run plus the fresh tools state, so
/// the page refreshes in one round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRunResult {
    pub id: String,
    /// The process exit code, or `None` if it was killed by a signal.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Whether the run exited cleanly (`exit_code == 0`).
    pub ok: bool,
    /// The run's combined stdout+stderr.
    pub output: String,
    pub state: ToolsState,
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
    /// Suggested models for this backend, shown as one-tap chips on the Model picker. The bare
    /// aliases / ids a user most often wants; anything else is still typed into the same field.
    #[serde(default)]
    pub model_suggestions: Vec<String>,
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
    /// A space-separated tool spec (as passed to `--allowed-tools`) edited two ways at once: a
    /// row of toggle chips for the well-known tools in `options`, over a free-text input for the
    /// same string, so scoped specifiers like `Bash(git *)` can still be typed by hand.
    ToolPicker,
    /// A single model value edited two ways at once: a row of single-select suggestion chips
    /// (the selected backend's `model_suggestions`) over a free-text input, so any other model
    /// alias or id can still be typed by hand.
    ModelPicker,
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
    /// The ids of the adi **tools** enabled for this agent (its per-tool checkboxes). Each becomes
    /// a shim in the agent's own `.bin` at launch. Named `bin_tools` to stay distinct from the LLM
    /// `--allowed-tools` in `arguments.tools`.
    #[serde(default)]
    pub bin_tools: Vec<String>,
    /// The secrets attached to this agent (its per-secret checkboxes). Each is a `(scope, name)`
    /// reference; at launch exactly these are decrypted and injected into the run's environment
    /// under their literal names — an explicit allowlist, never the whole scope.
    #[serde(default)]
    pub secrets: Vec<SecretRef>,
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
    /// The ids of the adi **tools** enabled for this agent (its per-tool checkboxes). Each becomes
    /// a shim in the agent's own `.bin` at launch. Named `bin_tools` to stay distinct from the LLM
    /// `--allowed-tools`.
    #[serde(default)]
    pub bin_tools: Vec<String>,
    /// The secrets to attach to this agent (its per-secret checkboxes). Each is a `(scope, name)`
    /// reference; only these are decrypted and injected into the agent's runs — an allowlist.
    #[serde(default)]
    pub secrets: Vec<SecretRef>,
    /// The agent's previous name when an edit renames it. The manifest is moved first (keeping
    /// `created_at`), then saved under `name`, so no orphan is left behind. Omitted — or equal to
    /// `name` — for a plain create/update.
    #[serde(default)]
    pub rename_from: Option<String>,
}

/// Request body naming an agent — `POST /api/agents/delete`, `/api/agents/stop`, `/api/agents/peek`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRef {
    pub name: String,
}

/// `POST /api/agents/run` request: the agent to launch and its initial task. The agent is only a
/// template — each launch is an independent run from those settings, never a continuation. Headless
/// backends (`process` / `harness`) run one `--print` turn with `message` as the prompt (required
/// there — see the handler); interactive (tmux) backends ignore it and type into the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAgent {
    pub name: String,
    #[serde(default)]
    pub message: String,
}

/// Request naming one specific run of an agent — `POST /api/agents/run/peek` and `/run/stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRef {
    pub name: String,
    pub run_id: String,
}

/// `POST /api/agents/run/reply` request — answer into one of a harness agent's conversations
/// (`run_id` is the conversation id), appending `message` as the next turn. Only harness backends
/// keep answerable conversations; anything else rejects it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyToRun {
    pub name: String,
    pub run_id: String,
    pub message: String,
}

/// One message in a harness conversation's transcript: a `user` question or an `assistant` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTurn {
    /// `"user"` or `"assistant"`.
    pub role: String,
    pub text: String,
    /// Unix milliseconds the turn was recorded (0 for the still-streaming answer).
    #[serde(default)]
    pub at: u64,
    /// True only for the provisional, still-streaming answer of a turn still in flight.
    #[serde(default)]
    pub pending: bool,
    /// The assistant turn's activity — tool calls and thinking — parsed from the engine's output.
    /// Empty for user turns and engines that emit no structured progress.
    #[serde(default)]
    pub steps: Vec<AgentStep>,
    /// The assistant turn's telemetry (tokens / cost / duration), when the engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<AgentTurnMetrics>,
}

/// A tool step's lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolStatus {
    Running,
    Ok,
    Error,
}

/// One activity step within an assistant turn — a tool call or a thinking block. The answer text is
/// not a step; it lives in [`AgentTurn::text`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentStep {
    /// A model reasoning block (shown dim/collapsed).
    Thinking { text: String },
    /// A tool invocation and, once it returns, its result.
    Tool {
        name: String,
        #[serde(default)]
        input: String,
        status: AgentToolStatus,
        #[serde(default)]
        output: String,
    },
}

/// Per-turn telemetry. Cost is in micro-dollars (1e-6 USD) so the whole model stays integer-exact.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTurnMetrics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
    #[serde(default)]
    pub permission_denials: Vec<String>,
    #[serde(default)]
    pub is_error: bool,
}

/// A backend's capability profile — the single source of truth the client renders from: which
/// container to show (pane / run history / chat) and which progress columns within it. Mirrors
/// `adi_agents::BackendCapabilities`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub interactive: bool,
    pub history: bool,
    pub answerable: bool,
    pub live_text: bool,
    pub tool_steps: bool,
    pub thinking: bool,
    pub metrics: bool,
}

/// One entry in a headless agent's run history: an independent run spawned from the agent's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunInfo {
    pub run_id: String,
    /// Unix milliseconds the run started.
    pub started_at: u64,
    /// The task the run was launched with.
    #[serde(default)]
    pub message: String,
    pub running: bool,
}

/// `POST /api/agents/runs` — a headless agent's run history, newest first. `interactive` is true for
/// tmux backends, which keep no run history (their live session is the run) and so return `runs: []`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRuns {
    pub name: String,
    #[serde(default)]
    pub interactive: bool,
    /// Whether these runs are *conversations* you can answer (harness backends) rather than one-shot
    /// runs — so the client shows a chat transcript + reply box instead of a plain log. Mirrors
    /// `caps.answerable`, kept for existing callers.
    #[serde(default)]
    pub answerable: bool,
    /// The backend's full capability profile — drives which container and progress columns to show.
    #[serde(default = "default_caps")]
    pub caps: AgentCapabilities,
    #[serde(default)]
    pub runs: Vec<AgentRunInfo>,
}

/// A zero capability profile — the `serde` default when an older/absent response omits `caps`.
fn default_caps() -> AgentCapabilities {
    AgentCapabilities {
        interactive: false,
        history: false,
        answerable: false,
        live_text: false,
        tool_steps: false,
        thinking: false,
        metrics: false,
    }
}

/// `POST /api/agents/run` — a human-readable launch outcome, the new run's id, and fresh agent state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub message: String,
    /// The id of the run just launched (empty for interactive backends, which have no run id).
    #[serde(default)]
    pub run_id: String,
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
    /// The command a human runs to follow the run: `tmux attach -t adi-agent-<name>` for an
    /// interactive session, or `tail -f <log>` for a headless detached run.
    #[serde(default)]
    pub attach: String,
    /// Whether this is an interactive (tmux) session — only then can the live view type into it.
    /// Headless `process` / `harness` runs are log-only, and their `output` persists after they end.
    #[serde(default)]
    pub interactive: bool,
    /// The run this snapshot is of, echoed back so a late poll for a run the view has moved off is
    /// dropped. Empty for interactive backends (a session, not a run).
    #[serde(default)]
    pub run_id: String,
    /// Whether this run is an answerable conversation (a harness backend). When true, `turns` carries
    /// its transcript and the client shows a chat with a reply box rather than the plain `output` log.
    /// Mirrors `caps.answerable`.
    #[serde(default)]
    pub answerable: bool,
    /// The backend's capability profile — drives the progress feed (which columns) for this run.
    #[serde(default = "default_caps")]
    pub caps: AgentCapabilities,
    /// The run/conversation transcript, oldest first — for backends that produce turns (conversations,
    /// and one-shot runs synthesized as a single answered turn); empty otherwise. Includes the
    /// still-streaming answer, with its parsed tool steps, while a turn is in flight.
    #[serde(default)]
    pub turns: Vec<AgentTurn>,
}

// ---- meta (the default ADI agent — a single well-known global agent) ----------------

/// `GET /api/meta` — the state of the Meta page, which manages one well-known global agent named
/// `adi-agent`: the default ADI agent (a "meta-agent" that helps set up and operate this
/// environment). The page reuses the agents endpoints (`/api/agents/save`, `/run`, `/peek`) to
/// create and run it, so this endpoint only reports whether it exists, its current definition, and
/// the canonical system prompt to seed a fresh one with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaState {
    /// The well-known agent name this page manages (`adi-agent`).
    pub name: String,
    /// The canonical system prompt a freshly created meta-agent is seeded with — it teaches the
    /// agent how to operate this ADI environment (the store, projects, services, dashboards,
    /// ports, DNS). The setup form opens prefilled with it, still editable.
    pub default_prompt: String,
    /// The `adi-agent` definition, or `None` when it hasn't been set up yet.
    #[serde(default)]
    pub agent: Option<AgentDto>,
    /// The agent create/edit form schema — its `backends` list drives the setup page's picker.
    pub form: AgentFormSpec,
}

// ---- triggers (code blocks launched by a webhook or supervised in the background) ----

/// One selectable trigger kind — *how* a trigger launches: `webhook` (an inbound HTTP call) or
/// `background` (a supervised long-lived process). Server-owned so the set can change without a
/// webapp rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerKindOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
}

/// One selectable runtime — what language a code block is written in (`sh`, `ts`) and therefore
/// which interpreter runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerRuntimeOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
}

/// One setting a preset's code block reads, offered as a labelled input in the editor and
/// exported to the code block as `ADI_<KEY>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerPresetField {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
    /// Prefilled when the preset is applied; empty when only the user can supply the value.
    #[serde(default)]
    pub default: String,
}

/// A ready-made trigger definition the editor can apply, prefilling the kind, runtime, code
/// block, and settings in one click. Applying one is a client-side prefill — nothing is stored
/// until the user saves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerPreset {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub kind: String,
    pub runtime: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub fields: Vec<TriggerPresetField>,
    /// For an event preset: the event-name patterns to prefill the subscription with. Empty for
    /// every other kind.
    #[serde(default)]
    pub events: Vec<String>,
}

/// One trigger definition, flattened for the wire. `kind` is how it launches, `runtime` is the
/// language of `code`. `last_fired_at` comes from the log's mtime; the `running`/`pid`/
/// `restarts` group describes a *background* trigger's supervised process and is inert for a
/// webhook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerDto {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub code: String,
    /// The preset this trigger was created from, if any — tells the editor which settings to
    /// offer when it is reopened.
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    /// The project this trigger is filed under (its id), or `None` for a global trigger.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
    /// For an event trigger: the event-name patterns it subscribes to (`adi.tasks.*`). Empty for
    /// the other kinds.
    #[serde(default)]
    pub events: Vec<String>,
    /// Restrict which projects may fire this trigger — an allowlist of project ids read from the
    /// fire's payload. Empty means unrestricted (fires for every project).
    #[serde(default)]
    pub trigger_on: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub last_fired_at: Option<u64>,
    /// Whether a supervisor is currently keeping this background trigger's process alive.
    #[serde(default)]
    pub running: bool,
    /// The live process's pid, while `running`.
    #[serde(default)]
    pub pid: Option<u32>,
    /// How long the live process has been up, in seconds.
    #[serde(default)]
    pub uptime_secs: Option<u64>,
    /// How many times the supervisor has relaunched it after an exit — non-zero means the code
    /// block keeps dying.
    #[serde(default)]
    pub restarts: u32,
}

/// `GET /api/triggers` — every registered trigger, sorted by name, plus the editor's
/// server-owned vocabulary: the kinds, the runtimes, and the preset catalog. Each mutation
/// endpoint returns a fresh one, so the client refreshes from one round-trip.
// Not `Eq`: `event_types` carries `serde_json::Value` schemas, which are `PartialEq` but not `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggersState {
    pub triggers: Vec<TriggerDto>,
    pub kinds: Vec<TriggerKindOption>,
    #[serde(default)]
    pub runtimes: Vec<TriggerRuntimeOption>,
    #[serde(default)]
    pub presets: Vec<TriggerPreset>,
    /// The catalog of platform events an `event` trigger can subscribe to — name, when it fires,
    /// the JSON Schema of the payload it delivers, and a concrete example. The editor shows these
    /// so a subscriber knows what to catch and exactly what shape to parse.
    #[serde(default)]
    pub event_types: Vec<EventTypeDto>,
}

/// One entry in the platform's event catalog: a concrete event name, when it fires, the JSON Schema
/// of its `ADI_PAYLOAD` body, and a concrete example instance. Mirrors `adi_events::EventType`; the
/// `schema` and `example` are reflected/serialized from the exact Rust type emitted at the source,
/// so they never drift from the real payload. Not `Eq` (the JSON values aren't).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventTypeDto {
    pub name: String,
    #[serde(default)]
    pub summary: String,
    /// The payload's JSON Schema — the authoritative structure a subscriber will parse.
    #[serde(default)]
    pub schema: serde_json::Value,
    /// A concrete example payload body (a real serialized instance of the schema's type).
    #[serde(default)]
    pub example: serde_json::Value,
}

/// Request body for `POST /api/triggers/save` — create or update a trigger definition (an
/// upsert keyed by `name`). `name` and `kind` are required. Timestamps are owned by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTrigger {
    pub name: String,
    pub kind: String,
    /// The language of `code` (`sh` / `ts`); omitted or unknown saves a shell block.
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub code: String,
    /// The preset this was prefilled from, recorded so the editor can re-offer its settings.
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "trigger_enabled_default")]
    pub enabled: bool,
    /// The project to file the trigger under (its id); blank/omitted saves a global trigger.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
    /// For an event trigger: the event-name patterns it subscribes to (`adi.tasks.*`). Blank
    /// entries are dropped server-side.
    #[serde(default)]
    pub events: Vec<String>,
    /// Restrict which projects may fire this trigger — an allowlist of project ids read from the
    /// fire's payload. Blank entries are dropped server-side; an empty list saves an unrestricted
    /// trigger (fires for every project).
    #[serde(default)]
    pub trigger_on: Vec<String>,
}

/// serde default for [`SaveTrigger::enabled`] — an omitted flag saves an enabled trigger.
fn trigger_enabled_default() -> bool {
    true
}

/// Request body for `POST /api/events/emit` — publish one platform event by hand, the way task
/// and agent mutations do automatically. Every enabled event trigger whose patterns match fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmitEvent {
    /// The dotted event name, e.g. `adi.tasks.created`.
    pub name: String,
    /// The event body handed to matching triggers as `ADI_PAYLOAD` (JSON by convention).
    #[serde(default)]
    pub payload: String,
}

/// Reply to a successful `POST /api/events/emit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmitAck {
    pub ok: bool,
    pub event: String,
}

/// Request body naming a trigger — `POST /api/triggers/delete`, `/fire`, and `/log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerRef {
    pub name: String,
}

/// `POST /api/triggers/fire` — the manual-fire outcome: a human-readable message (the spawned
/// pid), plus the fresh triggers state so the client refreshes in the same round-trip.
// Not `Eq`: it embeds `TriggersState`, whose event schemas are `serde_json::Value` (no `Eq`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

// ---- the ADI store browser (~/.adi/mono, jailed) ---------------------------------------

/// Request body for the store browser — `POST /api/fs/list` and `/api/fs/read`. `path` is
/// relative to the store root (`~/.adi/mono`); `""` is the root itself. The same [`adi_fs`]
/// jail rules as the project browser apply, so nothing outside the store is reachable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRef {
    /// The path within the store, relative to its root.
    #[serde(default)]
    pub path: String,
}

/// `POST /api/fs/list` — a directory listing within the ADI store, browsed through the
/// isolated [`adi_fs`] jail rooted at `~/.adi/mono`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsListing {
    /// The listed directory, relative to the store root (`""` is the root).
    pub path: String,
    /// The parent directory, or `None` at the root — so the UI can offer an "up" control
    /// without re-deriving it.
    #[serde(default)]
    pub parent: Option<String>,
    /// The directory's entries, sorted directories-first then case-insensitively by name.
    pub entries: Vec<FileEntry>,
}

/// `POST /api/fs/read` — one text file's contents, read through the store jail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsContent {
    /// The file path, relative to the store root.
    pub path: String,
    /// The file's UTF-8 text (binary files are rejected rather than returned here).
    pub content: String,
    /// The file size in bytes.
    pub size: u64,
    /// Last-modified time as Unix epoch seconds, when the platform reports it.
    #[serde(default)]
    pub modified: Option<u64>,
}

/// Request body for saving a file in the store — `POST /api/fs/write`. Writes are atomic and
/// create any missing parent directories within the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsWrite {
    /// The file path to write, relative to the store root.
    pub path: String,
    /// The new UTF-8 text contents.
    pub content: String,
}

/// `POST /api/fs/create` — create one empty file or directory within the ADI store. Creates
/// never clobber; an existing path is a 409 rather than an overwrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsCreate {
    /// The path to create, relative to the store root.
    pub path: String,
    /// What to create: `"dir"` for a directory, anything else (`"file"`) for an empty file.
    pub kind: String,
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
    /// When the dashboard was archived (Unix seconds), or `None` while it is live. Archiving
    /// takes both bun services out of the supervisor's import glob (so they stop) and hides the
    /// row behind the Archived disclosure — without deleting any of the dashboard's files.
    #[serde(default)]
    pub archived_at: Option<u64>,
}

impl Dashboard {
    /// Whether the dashboard is archived (soft-removed from supervision).
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
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

/// Request body naming a dashboard — `POST /api/dashboards/archive` and `/unarchive`. Both
/// return a fresh [`DashboardsState`], so the client refreshes the listing in one round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardRef {
    pub id: String,
}

// MARK: secrets — encrypted global / per-project key-values (~/.adi/mono/secrets)

/// One secret's **metadata** — `GET /api/secrets` returns a list of these across every scope.
/// It never carries the value: the plaintext is only ever returned by an explicit reveal
/// (`POST /api/secrets/reveal` → [`RevealedSecret`]), so listing can't leak it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretDto {
    /// The project this secret is scoped to, or `None` for a global secret.
    #[serde(default)]
    pub project: Option<String>,
    /// The secret's key name (also the env-var name it injects into runs as).
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Present when the value came from an OAuth flow — provider, lifetime, and whether a refresh
    /// token is held. **Never a token.** `None` for a plain text secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthInfoDto>,
}

/// The non-secret OAuth provenance of a secret, for display: provider, token lifetime, and
/// whether a refresh token is held. Never carries a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthInfoDto {
    pub provider: String,
    pub obtained_at: u64,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
    pub has_refresh: bool,
}

/// Request body storing a secret obtained from an OAuth flow — `POST /api/secrets/set-oauth`.
/// The browser posts the tokens it received in the redirect fragment; the server encrypts the
/// access token as the value, encrypts the refresh token separately, and records the metadata.
/// `expires_in` is the provider's seconds-to-expiry; the server stamps the absolute `expires_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetOAuthSecret {
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub provider: String,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// `GET /api/secrets` — every secret across all scopes (metadata only). Each mutation endpoint
/// returns a fresh one of these, so the client refreshes from one round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretsState {
    pub secrets: Vec<SecretDto>,
}

/// Request body setting a secret — `POST /api/secrets/set`. `project` omitted/blank = global.
/// The plaintext `value` travels here (client → server on localhost) to be encrypted at rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetSecret {
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Request body naming a secret in a scope — `POST /api/secrets/remove` and `/reveal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
}

/// `POST /api/secrets/reveal` response — the one place a decrypted value crosses the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevealedSecret {
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    pub value: String,
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
