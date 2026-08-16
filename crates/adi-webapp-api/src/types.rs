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

/// What the process behind a port is costing the machine, rolled up over its whole process
/// tree — a service is usually a shell that spawned the real server, so charging the listener
/// alone would under-report almost everything.
///
/// Sampled by the host from the OS process table; absent when it could not be read (no `ps`,
/// or the process exited between the port scan and the sample).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessUsage {
    /// The process the numbers are rooted at — the one holding the port.
    pub pid: u32,
    /// CPU share as a percentage of **one** core, summed over the tree, so a busy
    /// multi-threaded service can exceed 100.
    pub cpu_percent: f32,
    /// Resident set size in bytes, summed over the tree.
    pub memory_bytes: u64,
    /// How many processes the sample covers: the listener plus its descendants.
    pub processes: u32,
    /// How long the listener has been up, in seconds.
    pub uptime_secs: u64,
}

/// One TCP port observed in the `LISTEN` state on the machine, with the owning process
/// where the OS reports it. Whether it's ADI-managed is decided by the client, which joins
/// these against the registry [`Lease`]s by port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsedPort {
    pub port: u16,
    pub process: Option<String>,
    pub pid: Option<u32>,
    /// What that process tree currently costs, when the host could sample it.
    #[serde(default)]
    pub usage: Option<ProcessUsage>,
}

/// `GET /api/ports/used` — every listening TCP port on the machine, sorted by port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

// ---- fleet (the remote adi nodes paired with this machine) --------------------------

/// `GET /api/fleet` — every node paired with this machine, in petname order. Every mutation
/// endpoint answers with a fresh one of these, so the panel updates in one round-trip (the
/// contract `/api/mesh` already keeps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetState {
    pub nodes: Vec<FleetNode>,
}

/// One paired node: all three of its names (`docs/fleet.md` §2) and what it may reach here (§5).
///
/// **The verifier never leaves the machine.** The registry stores each node's Basic-auth
/// credential as a salted digest beside its salt; this carries only a `has_password` flag —
/// enough for the panel to say the gate is configured, and nothing an offline cracker could work
/// against. A DTO is the wire, and the wire is exactly where a verifier must not be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetNode {
    /// What *this* machine calls the node: local, unique, and the label in
    /// `<service>.<petname>.n.adi`.
    pub petname: String,
    /// The node's `EndpointId` — the identity of record, and the only thing authorization is
    /// ever decided by. Full, because it is what an operator confirms out of band; see
    /// [`key_short`](Self::key_short) for the rendering a table cell wants.
    pub key: String,
    /// What the node calls *itself*, as acknowledged here. Only ever a suggestion.
    pub nickname: String,
    /// Unix seconds at which petname→key was pinned (trust on first use).
    pub paired_at: u64,
    /// What this node may reach here, in the string form an operator types: `http:*`,
    /// `http:nosh`, `tcp:127.0.0.1:22`, `ctl:read`. **Empty denies everything.**
    pub grants: Vec<String>,
    /// Whether a Basic-auth credential is configured for this node — the password its requests
    /// into *this* machine must carry, and the human-scoped half of §5's gate (the grant above is
    /// the machine-scoped half). Never the digest, never the salt.
    pub has_password: bool,
    /// A newer nickname the node has declared that this machine has *not* acknowledged. A
    /// notification, never a re-point: §2 rule 4 exists so no node can rename itself into
    /// another's links, and ignoring this line is exactly the case that rule guards.
    #[serde(default)]
    pub pending_nickname: Option<String>,
}

impl FleetNode {
    /// The key shortened for a table cell — head and tail around an ellipsis, so two keys that
    /// differ are still visibly different. One implementation, here rather than in the page, so
    /// the panel and anything else rendering a fleet abbreviate a key the same way.
    #[must_use]
    pub fn key_short(&self) -> String {
        // By characters, not bytes: a hand-edited `fleet.toml` can hold anything, and a panel
        // that panics on a stray non-ASCII key is worse than one that renders it in full.
        let chars: Vec<char> = self.key.chars().collect();
        if chars.len() <= 16 {
            return self.key.clone();
        }
        let head: String = chars[..8].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}\u{2026}{tail}")
    }

    /// This node's control panel: `app.<petname>.n.adi` (§1). The `n.adi` suffix is reserved for
    /// remote nodes, so this can never collide with a local `<service>.adi`.
    #[must_use]
    pub fn app_host(&self) -> String {
        format!("app.{}.n.adi", self.petname)
    }

    /// Whether the node declares a nickname this machine has not acknowledged.
    #[must_use]
    pub fn has_pending_nickname(&self) -> bool {
        self.pending_nickname.is_some()
    }
}

/// Request body naming one node — `POST /api/fleet/unpair` and the two nickname endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetRef {
    pub petname: String,
}

/// Request body for the local rename — `POST /api/fleet/rename`. Local because the far side is
/// not involved: this is §2 rule 5's escape hatch for two fleets that both use `main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetRename {
    /// The node's current petname.
    pub petname: String,
    /// The petname it should answer to here from now on.
    pub to: String,
}

/// Request body naming one grant on one node — `POST /api/fleet/grants/add` and
/// `/api/fleet/grants/remove`. The grant is its string form; the server parses it, and an
/// unparseable one is a 400 rather than a silently dropped rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetGrantRef {
    pub petname: String,
    pub grant: String,
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
///
/// A service runs one of two runner kinds: a **script** (`run` is the shell command) or a
/// **docker** container (`docker` is set). Set exactly one — `docker` wins if both are given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewService {
    /// The owning project id (services are always project-scoped; the global front-door
    /// hive is hand-edited, not API-managed).
    pub project: String,
    /// The service name — the key under `services:` and the ports-manager lease segment.
    pub name: String,
    /// The runner command (`runner.script.run`), executed via `sh -c`. Required for a script
    /// runner; ignored (may be empty) when `docker` is set.
    #[serde(default)]
    pub run: String,
    /// The proxied host (`proxy.host`, e.g. `demo.adi`); omitted → no front-door route.
    #[serde(default)]
    pub host: Option<String>,
    /// An explicit `http` port; omitted → a `` ports-manager.get('<project>/<name>', 'http') ``
    /// command is written instead, so the port is leased on read. This is the **host** port —
    /// for a docker runner it maps to the container port in [`NewServiceDocker::container_port`].
    #[serde(default)]
    pub port: Option<u16>,
    /// The runner's working directory, relative to the project dir (`runner.script.working_dir`).
    /// Script runner only.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Restart policy (`always` | `on-failure` | `no`); omitted → adi-hive's default.
    #[serde(default)]
    pub restart: Option<String>,
    /// When set, the service is a **Docker container** runner (`runner.docker`) rather than a
    /// script — see [`NewServiceDocker`].
    #[serde(default)]
    pub docker: Option<NewServiceDocker>,
}

/// The docker-runner half of a [`NewService`] — the fields the create form collects for a
/// container service (`runner.docker`). Mirrors adi-hive's `Docker` config; only `image` is
/// required. Host ports stay adi-hive's job (the service's leased `http` port); `container_port`
/// is the port inside the container that host `http` port forwards to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewServiceDocker {
    /// The image to run, e.g. `nginx:1.27` (required).
    pub image: String,
    /// The container port the service's `http` host port forwards to (`docker.ports.http`).
    #[serde(default)]
    pub container_port: Option<u16>,
    /// Bind mounts, compose `host:container[:mode]` syntax (relative host paths resolve against
    /// the project dir).
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Extra container environment (`docker.environment`), as `KEY=VALUE` entries.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Image pull policy (`always` | `missing` | `never`), or omitted for docker's default.
    #[serde(default)]
    pub pull: Option<String>,
    /// Raw extra `docker run` flags (`docker.args`) — the escape hatch for anything not modelled.
    #[serde(default)]
    pub args: Vec<String>,
    /// Override the image's default command (`docker.command`), appended after the image.
    #[serde(default)]
    pub command: Vec<String>,
}

/// One named port a service declares (`rollout.recreate.ports.<key> = <port>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServicePort {
    pub key: String,
    pub port: u16,
}

/// A service read from a project's `.adi/hive.yaml` — a read-only summary for the detail view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// What the process holding that port costs right now; `None` when the service is down
    /// or the host could not sample it.
    #[serde(default)]
    pub usage: Option<ProcessUsage>,
}

/// `GET /api/projects/<id>` — one project's manifest plus the services parsed from its
/// `.adi/hive.yaml` ("inside" the project). `has_hive` distinguishes "no hive.yaml" from
/// "hive.yaml with no services".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Where this task's work happens — the directory a run picking it up should start in, or
    /// `None` when the task has no opinion and the run starts where its agent is defined to.
    #[serde(default)]
    pub cwd: Option<String>,
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
    /// Where this task's work happens (`~` allowed). Omitted, a subtask inherits its parent's.
    #[serde(default)]
    pub cwd: Option<String>,
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
    /// The ready-made ways to stand an agent up, in the order the setup page offers them. The
    /// onboarding wizard renders these instead of a bare backend picker; the last one is the
    /// manual escape hatch onto the full field list.
    #[serde(default)]
    pub presets: Vec<AgentSetupPreset>,
}

/// One ready-made way to stand an agent up — "Claude Code SDK", "Kimi API key" — holding
/// everything that choice implies: the backend it saves as, the arguments it pins without asking,
/// the handful of fields still worth a question, and the API key it stores as a secret. The last
/// preset is [`AgentSetupPreset::manual`]: it pins nothing and hands the user the whole schema.
///
/// Server-owned because every value in it is backend knowledge — which provider id, which
/// environment variable holds its key, which endpoint it lives at. A client that hardcoded those
/// would drift from the backends the moment one moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSetupPreset {
    pub id: String,
    pub label: String,
    /// One line under the picker: what this choice actually runs, and on whose credentials.
    pub blurb: String,
    /// The backend id this preset saves as. Empty on the manual preset, which asks instead.
    #[serde(default)]
    pub backend: String,
    /// The arguments this preset writes. A key that also appears in `fields` is a **prefill** the
    /// user can still edit; every other key is pinned and never shown.
    #[serde(default)]
    pub arguments: BTreeMap<String, String>,
    /// The fields to ask for, named from [`AgentFormSpec::fields`] and rendered in this order.
    /// Empty on the manual preset, which renders everything the chosen backend takes.
    #[serde(default)]
    pub fields: Vec<String>,
    /// The API key this preset stores as a global secret and attaches to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<AgentSetupSecret>,
    /// Whether this preset drops the user into the full schema-driven form — the backend picker
    /// and every field that backend takes — instead of its own short list.
    #[serde(default)]
    pub manual: bool,
}

/// The API key a preset asks for: the environment variable the backend reads it from (which is
/// also the secret's name), and the copy for its input. The value itself is stored through
/// `POST /api/secrets/set` and attached to the agent — it never rides along in the agent manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSetupSecret {
    /// The environment variable the backend reads the key from — also the secret's name.
    pub env: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub placeholder: String,
    /// Whether the agent cannot run without it. `false` where a CLI login is the other way in.
    #[serde(default)]
    pub required: bool,
}

/// One selectable agent backend in the form: an `executor:what` pair, where the executor is the
/// run mechanism (`pty` / `process` / `harness` / `wasm`) and the suffix is what it runs.
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
    /// The knowledge bases this agent works with, written as `global/<name>`,
    /// `project:<id>/<name>`, or `agent:<name>/<base>`. A wish list rather than a grant: what the
    /// agent may actually reach is decided by the three isolation levels at read time.
    #[serde(default)]
    pub knowledge: Vec<String>,
    /// Whether the agent keeps a memory of its own — the `agent:<name>/memory` base, which it
    /// alone writes and every other agent may read.
    #[serde(default)]
    pub memory: bool,
    /// The secrets attached to this agent (its per-secret checkboxes). Each is a `(scope, name)`
    /// reference; at launch exactly these are decrypted and injected into the run's environment
    /// under their literal names — an explicit allowlist, never the whole scope.
    #[serde(default)]
    pub secrets: Vec<SecretRef>,
    /// Extra directories prepended to the run's `PATH`, ahead of the machine's own — how an agent
    /// pins a toolchain (a project's nvm node, say) that the default `PATH` doesn't point at. `~`
    /// and `$HOME` are expanded at launch.
    #[serde(default)]
    pub path: Vec<String>,
    /// Extra environment variables for the run, under their literal names. `PATH` is not settable
    /// here — it is built from `path` above.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Whether this agent runs with nobody watching. It changes one thing: the `Ask` tool refuses,
    /// telling the run to decide for itself and say what it assumed — because a question nobody
    /// will see is the work stopping without failing.
    #[serde(default)]
    pub unattended: bool,
    /// Whether this agent's backend has a run adapter, i.e. whether ▶ Run can work at all.
    #[serde(default)]
    pub runnable: bool,
    /// Whether this agent has a live pty session or detached process right now.
    #[serde(default)]
    pub running: bool,
    /// Whether a run of *this* agent would be refused right now — the global cap is full, or its
    /// project's is. The client uses it to offer "Run anyway" instead of walking into a 429.
    #[serde(default)]
    pub at_run_limit: bool,
    /// What this agent's backend can do. Carried by the *listing* and not only by an open run,
    /// because some of it has to be known before a run exists: whether the composer that starts a
    /// conversation offers to attach an image is decided while the box is still empty.
    #[serde(default = "default_caps")]
    pub caps: AgentCapabilities,
}

/// `GET /api/agents` — every registered agent definition, sorted by name. Each mutation endpoint
/// returns a fresh one, so the client refreshes from one round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentsState {
    pub agents: Vec<AgentDto>,
    pub form: AgentFormSpec,
    /// How many runs may be live at once before an unforced launch is refused; `0` means no limit.
    #[serde(default)]
    pub max_concurrent_runs: u32,
    /// How many runs are live right now, across every agent — the number weighed against the limit,
    /// so the client can say "at the limit" before it asks.
    #[serde(default)]
    pub running_runs: u32,
    /// The per-project caps and loads: one entry for every project that has a cap of its own or
    /// something running. A project not listed here has no cap of its own and nothing live.
    #[serde(default)]
    pub project_run_limits: Vec<ProjectRunLimit>,
}

/// One project's slice of the run caps: what it is allowed and what it is using.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRunLimit {
    pub project: String,
    /// This project's own cap; `0` means it has none and is bounded only by the global limit.
    #[serde(default)]
    pub max_concurrent_runs: u32,
    /// How many of this project's runs are live right now.
    #[serde(default)]
    pub running_runs: u32,
}

/// Request body for `POST /api/agents/limit` — set how many runs may be live at once. With
/// `project` set it is that project's own cap (`0` clears it, leaving only the global limit);
/// without, the global one (`0` lifts it). A project cap narrows the global number, never lifts it.
/// Both bind automatic launches (a trigger, a queued chat turn); a human overrides them per run
/// with `force`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRunLimit {
    pub max_concurrent_runs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
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
    /// **Omit to keep whatever the agent already has**, exactly as for `bin_tools` below; send an
    /// empty list to actually clear. Only the full agent editor and onboarding offer the field,
    /// and a save from a form that doesn't must not strip an agent's tags by saying nothing.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Omitted means unchanged, for the same reason as `tags`.
    #[serde(default)]
    pub starred: Option<bool>,
    /// The project to file the agent under (its id). **Omit to keep whatever the agent already
    /// has**; send a blank string to make it global. A project is not a label — it decides which
    /// database, secrets, and knowledge bases the agent's runs reach — so unfiling one has to be
    /// something a request *said*, never something it failed to mention.
    #[serde(default)]
    pub project: Option<String>,
    /// The ids of the adi **tools** enabled for this agent (its per-tool checkboxes). Each becomes
    /// a shim in the agent's own `.bin` at launch. Named `bin_tools` to stay distinct from the LLM
    /// `--allowed-tools`. **Omit to keep whatever the agent already has**, exactly as for `path`
    /// and `env` below — a form that doesn't offer the tool checkboxes must not un-tick them by
    /// saying nothing. Send an empty list to actually clear.
    #[serde(default)]
    pub bin_tools: Option<Vec<String>>,
    /// The knowledge bases this agent works with (see [`AgentDto::knowledge`]). **Omit to keep
    /// whatever the agent already has**, for the same reason as `bin_tools`; send an empty list
    /// to clear.
    #[serde(default)]
    pub knowledge: Option<Vec<String>>,
    /// Whether the agent keeps its own memory (see [`AgentDto::memory`]). Omitted means
    /// unchanged — a save from a form that never offered the toggle must not take an agent's
    /// memory away.
    #[serde(default)]
    pub memory: Option<bool>,
    /// The secrets to attach to this agent (its per-secret checkboxes). Each is a `(scope, name)`
    /// reference; only these are decrypted and injected into the agent's runs — an allowlist.
    /// **Omit to keep whatever the agent already has**, as for `bin_tools`; send an empty list to
    /// detach them all. A save that silently detached them would leave the agent's runs missing
    /// the credentials they were built around.
    #[serde(default)]
    pub secrets: Option<Vec<SecretRef>>,
    /// Extra directories for the run's `PATH` (see [`AgentDto::path`]). **Omit to keep whatever the
    /// agent already has** — only the full agent editor sends these, so a form that doesn't offer
    /// them (the meta setup, the project panel) can save without silently clearing them. Send an
    /// empty list to actually clear.
    #[serde(default)]
    pub path: Option<Vec<String>>,
    /// Extra environment variables for the run (see [`AgentDto::env`]). Omitted means unchanged,
    /// exactly as for `path` above; an empty table clears.
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    /// Whether the agent runs unattended (see [`AgentDto::unattended`]). **Omit to keep whatever
    /// the agent already has** — only the full agent editor offers it, so the meta setup and the
    /// project panel can save without silently switching it off.
    #[serde(default)]
    pub unattended: Option<bool>,
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
/// there — see the handler); interactive (pty) backends ignore it and type into the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAgent {
    pub name: String,
    #[serde(default)]
    pub message: String,
    /// Images to attach to the opening message, by the ids `POST /api/agents/attachment` minted.
    ///
    /// Ids rather than bytes because the bytes are already on the server: a composer uploads a
    /// screenshot the moment it is pasted, so that a slow upload happens while the message is still
    /// being typed rather than after Send is pressed. An id that no longer resolves is one fewer
    /// image, not a refused launch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// Where *this* run starts, overriding the directory its manifest implies. For a caller that
    /// points one agent definition at a different target each launch — a recon pass, a per-repo
    /// reviewer — where no stored field can hold the answer. Absent/blank leaves the manifest and
    /// the agent's project to decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Launch even when the concurrency limit is full — a human's deliberate "run it anyway", after
    /// a refusal or straight from a UI that can already see the cap is full. Never set by anything
    /// the platform starts on its own.
    #[serde(default)]
    pub force: bool,
}

/// Request naming one specific run of an agent — `POST /api/agents/run/peek` and `/run/stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRef {
    pub name: String,
    pub run_id: String,
}

/// `POST /api/agents/run/hide` request — hide one session from the chat rail, or (`hidden: false`)
/// bring it back. Nothing about the run changes: it keeps running, and keeps its log and transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HideRun {
    pub name: String,
    pub run_id: String,
    pub hidden: bool,
}

/// `POST /api/agents/run/reply` request — say `message` into one of a harness agent's conversations
/// (`run_id` is the conversation id). It becomes the next turn, or — while the agent is still
/// answering — waits in that conversation's queue. Only harness backends keep answerable
/// conversations; anything else rejects it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyToRun {
    pub name: String,
    pub run_id: String,
    pub message: String,
    /// Images attached to this reply, by the ids `POST /api/agents/attachment` minted — see
    /// [`RunAgent::attachments`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
}

/// One image attached to a message: enough to draw it, not the bytes themselves.
///
/// The bytes are fetched once from `GET /api/agents/attachment/<id>`. A transcript is polled every
/// second, and one that inlined its images would re-send every screenshot in the conversation on
/// every tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAttachment {
    /// What to fetch the bytes by, and what to name it in a send.
    pub id: String,
    /// The original filename, for the reader — a pasted screenshot is named by whoever pasted it.
    #[serde(default)]
    pub name: String,
    /// `image/png`, `image/jpeg`, `image/webp` or `image/gif`.
    #[serde(default)]
    pub media_type: String,
    /// Bytes on disk, so a client can show the size without asking for the file.
    #[serde(default)]
    pub size: u64,
}

/// `POST /api/agents/run/answer` request — settle the question a conversation is waiting on, with
/// one reply per question in the order they were asked.
///
/// `ask` names the ask being answered so a card left open in another tab cannot settle the question
/// that has since replaced it; omit it to answer whatever is pending. Either way exactly one caller
/// wins — the loser gets a 404 rather than starting a second turn on a settled question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerRun {
    pub name: String,
    pub run_id: String,
    #[serde(default)]
    pub ask: Option<String>,
    pub replies: Vec<String>,
}

/// One question a run stopped to ask, on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentQuestion {
    /// Two or three words naming the decision — the chip beside the question.
    #[serde(default)]
    pub header: String,
    pub question: String,
    /// The answers offered as one tap. Empty means free text only.
    #[serde(default)]
    pub options: Vec<AgentChoice>,
    #[serde(default)]
    pub multi_select: bool,
}

/// One offered answer: what the button says, and what choosing it implies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentChoice {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// A run's request for a human decision — everything the card needs to draw itself.
///
/// Carried on [`AgentRunInfo`] (so a listing can badge the conversations that need somebody) and on
/// [`AgentPeek`] (so the open chat can draw the card). Present only while it is unanswered: a
/// settled ask is history, and history is the transcript's job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAsk {
    pub id: String,
    /// Unix milliseconds it was asked.
    #[serde(default)]
    pub asked_at: u64,
    /// What the run was doing when it stopped to ask, in its own words.
    #[serde(default)]
    pub note: String,
    pub questions: Vec<AgentQuestion>,
    /// Unix milliseconds after which the run's own defaults are taken instead, if it named a
    /// deadline. The client counts down against it — an answer that arrives after this has no
    /// question left to settle.
    #[serde(default)]
    pub deadline: Option<u64>,
    /// The first question plus how many came with it: one line, fit for a rail badge.
    #[serde(default)]
    pub headline: String,
}

/// One row of the cross-agent "needs you" inbox: an unanswered ask, and enough about the
/// conversation it came from to open it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAsk {
    pub agent: String,
    /// The conversation blocked on it — the `run_id` every other agent endpoint takes.
    pub run_id: String,
    /// That conversation's title (the message it was opened with, cut short).
    #[serde(default)]
    pub conversation: String,
    pub ask: AgentAsk,
}

/// `GET /api/agents/questions` — every unanswered question across every agent, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAsks {
    pub asks: Vec<PendingAsk>,
}

/// One goal on a conversation: what would make it done, and where it got to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentGoal {
    pub id: String,
    pub agent: String,
    /// The conversation it is nudged into — the `run_id` every other agent endpoint takes.
    pub run_id: String,
    pub text: String,
    /// `open`, `met`, or `given_up`.
    pub state: String,
    /// `human` or `agent` — whether somebody set this or the run set it for itself.
    #[serde(default)]
    pub set_by: String,
    #[serde(default)]
    pub created_at: u64,
    /// How many times the conversation has been asked about it. Shown because nothing closes a goal
    /// on the run's behalf, so a number that keeps climbing is the only sign of a run circling one.
    #[serde(default)]
    pub nudges: u64,
    #[serde(default)]
    pub closed_at: Option<u64>,
    /// The evidence a `met` carried, or the reason a give-up did.
    #[serde(default)]
    pub note: String,
}

/// `POST /api/agents/goals` request — one conversation's goals, or (with neither field) every open
/// goal on the machine.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GoalsOf {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub run_id: String,
}

/// `POST /api/agents/goal/set` request — write a goal onto a conversation, or reword one.
///
/// `goal` names an existing goal to reword; without it this creates a new one. A conversation may
/// carry several, so creating never replaces.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SetGoal {
    pub name: String,
    pub run_id: String,
    pub text: String,
    #[serde(default)]
    pub goal: Option<String>,
}

/// `POST /api/agents/goal/close` request — close a goal, one way or the other.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CloseGoal {
    pub goal: String,
    /// `met` closes it as done; anything else closes it as given up on. Two spellings rather than a
    /// boolean because the caller is naming an action, and `met: false` is not what giving up means.
    #[serde(default)]
    pub as_: String,
    /// The evidence, or the reason.
    #[serde(default)]
    pub note: String,
}

/// The answer to every goal endpoint: the goals in question, newest state included.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentGoals {
    pub goals: Vec<AgentGoal>,
}

/// `POST /api/agents/run/unqueue` request — drop the message at `index` from a conversation's queue,
/// before it is ever asked. Out-of-range is a no-op, not an error: a queued message that started its
/// turn between the click and the request is simply gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnqueueFromRun {
    pub name: String,
    pub run_id: String,
    pub index: usize,
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
    /// True only for a user message still waiting in the conversation's queue — typed while the
    /// agent was answering, and not yet asked. Its index among the queued turns is its place in the
    /// queue, which is what `/api/agents/run/unqueue` takes.
    #[serde(default)]
    pub queued: bool,
    /// The images this message carries, in the order they were attached. Only ever on a user turn —
    /// pictures travel from the person to the model, never back.
    #[serde(default)]
    pub images: Vec<AgentAttachment>,
    /// The assistant turn's activity — tool calls and thinking — parsed from the engine's output.
    /// Empty for user turns and engines that emit no structured progress.
    #[serde(default)]
    pub steps: Vec<AgentStep>,
    /// The assistant turn's telemetry (tokens / cost / duration), when the engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<AgentTurnMetrics>,
}

/// A tool step's lifecycle status. `unanswered` is a call the run ended on top of — it went out and
/// nothing ever came back, which is not the same as a tool that answered with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolStatus {
    Running,
    Ok,
    Error,
    Unanswered,
}

/// One item on an assistant turn's timeline, in the order it happened: something the agent said, a
/// thinking block, or a tool call. The turn's *final* message is not a step — it lives in
/// [`AgentTurn::text`]. Mirrors `adi_agents::Step`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentStep {
    /// Something the agent said mid-turn, between tool calls.
    Message { text: String },
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
    /// Whether a run of this backend can stop and ask a person something. True wherever
    /// `answerable` is — asking is answering in reverse, and a backend with no thread to continue
    /// has nowhere to deliver an answer into.
    #[serde(default)]
    pub asks: bool,
    /// Whether a message to this backend may carry images — what decides whether the composer offers
    /// to attach one at all. False for engines handed their message as a command-line argument,
    /// where a picture has no representation.
    #[serde(default)]
    pub images: bool,
}

/// One entry in a headless agent's run history: an independent run spawned from the agent's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunInfo {
    pub run_id: String,
    /// Unix milliseconds the run started.
    pub started_at: u64,
    /// Unix milliseconds the run last *said* something — the moment on the last turn of its
    /// transcript, and what "recently active" is judged on. A chat being opened, read, or spooled
    /// into never moves it; only a message does. Never earlier than `started_at`; defaults to 0 only
    /// when an older server omits it, and a client should then read it as `started_at`.
    #[serde(default)]
    pub last_activity: u64,
    /// The task the run was launched with, **cut to a title** (300 characters) — a listing carries
    /// four hundred of these and a rail shows 72 characters of one. The whole message is the
    /// conversation's first turn, so a reader that wants it opens the chat.
    #[serde(default)]
    pub message: String,
    pub running: bool,
    /// Whether this session has been hidden from the chat rail (`POST /api/agents/run/hide`). The run
    /// is listed either way — hiding is a listing preference the client applies, not a filter the
    /// server does — so a full history view can still show it.
    #[serde(default)]
    pub hidden: bool,
    /// The question this conversation is waiting on a person for, if it is waiting on one.
    ///
    /// Carried by the *listing* and not only by the open chat, because "which of these needs me?"
    /// is the question a rail of forty conversations exists to answer. `running: false` alone
    /// cannot: finished and blocked-on-you look identical from there, and they are opposites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_question: Option<AgentAsk>,
    /// What became of the run, once it has stopped — the engine's own verdict, carried by the
    /// listing for the same reason `pending_question` is: `running: false` says a run is over and
    /// nothing at all about whether it worked, and the alternative is opening every one to find out.
    /// `None` while it runs, and for runs that ended before the store began keeping this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentRunOutcome>,
}

/// How a run ended, as reported by whatever engine ran it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRunOutcome {
    /// The engine's own word for how it ended (`completed`, `api_error`, `aborted_tools`, …).
    /// Absent from an engine that reports no such thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    /// Whether the engine called it a failure — the one field to branch on without knowing any
    /// particular engine's vocabulary.
    #[serde(default)]
    pub is_error: bool,
    /// Cost in micro-dollars (1e-6 USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
    /// The opening of what the run answered.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_head: String,
    /// Unix milliseconds the ending was noticed — later than when it happened, for a run nobody
    /// was watching.
    #[serde(default)]
    pub noted_at: u64,
}

/// `POST /api/agents/runs` — a headless agent's run history, newest first. `interactive` is true for
/// pty backends, which keep no run history (their live session is the run) and so return `runs: []`.
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

/// `GET /api/agents/runs/all` — the run history of **every** agent, in one round-trip, for the
/// cross-agent chat index. One [`AgentRuns`] per agent (same shape as `/api/agents/runs`), so the
/// client can flatten them into a single "all chats" list and open any conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllAgentRuns {
    pub agents: Vec<AgentRuns>,
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
        asks: false,
        images: false,
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

/// `POST /api/agents/run/review` request — hand one conversation to a reviewing agent.
///
/// A [`RunRef`] plus who should read it. `reviewer` is normally left blank, meaning the
/// environment's root agent (`adi-agent`); it is named only when an operator keeps a reviewer of
/// their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRun {
    pub name: String,
    pub run_id: String,
    /// Who reviews. Blank/absent means the root agent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reviewer: String,
}

/// `POST /api/agents/run/review` — where the review is being written, and where its evidence went.
///
/// The client's whole job with this is to open `reviewer`/`run_id`: the review *is* a conversation,
/// so there is no report to render here, only somewhere to go and watch one being written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReviewStarted {
    /// The agent now doing the reviewing.
    pub reviewer: String,
    /// Its new conversation. Empty for an interactive reviewer, which keeps no run history — the
    /// client then opens the agent rather than a session of it.
    #[serde(default)]
    pub run_id: String,
    /// Where the dossier was written, so a reader can open the evidence themselves.
    pub dossier: String,
    /// The conversation that was reviewed, echoed back so a late reply is matched to the chat it
    /// was asked from rather than whichever one is open when it lands.
    pub reviewed: RunRef,
}

/// Request body for `POST /api/agents/send-keys` — type into a running agent's pty session:
/// `text` is sent literally, then `key` (a key name: `Enter`, `Escape`, `Up`, `C-c`, …)
/// is pressed. At least one of the two must be non-empty. Replies with a fresh [`AgentPeek`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKeys {
    pub name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub key: String,
}

/// `POST /api/agents/peek` — a read-only snapshot of a running agent's pty screen (the text the
/// live view shows), polled by the Agents page's live view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPeek {
    pub name: String,
    /// Whether the agent's pty session is live; `output` is empty when it isn't.
    pub running: bool,
    /// The visible pane text (trailing whitespace trimmed).
    #[serde(default)]
    pub output: String,
    /// The command a human runs to follow the run: empty for an interactive pty session (viewed
    /// in the control panel, no external attach), or `tail -f <log>` for a headless detached run.
    #[serde(default)]
    pub attach: String,
    /// Whether this is an interactive (pty) session — only then can the live view type into it.
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
    /// The question this conversation is waiting on a person for, if any — what the chat draws its
    /// card from. Gone the moment it is answered; the answer itself is a turn like any other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_question: Option<AgentAsk>,
    /// The run/conversation transcript, oldest first — for backends that produce turns (conversations,
    /// and one-shot runs synthesized as a single answered turn); empty otherwise. Includes the
    /// still-streaming answer, with its parsed tool steps, while a turn is in flight.
    #[serde(default)]
    pub turns: Vec<AgentTurn>,
}

// ---- conversation token analytics --------------------------------------------------
// A turn's metrics say what it cost. These say what it was spent *on* — and, mostly, what was paid
// for twice. Computed by re-reading the transcript through a tokenizer, so it is a request the
// reader makes rather than something folded into the one-second poll.

/// Where a piece of a conversation came from. Mirrors `adi_agents::analytics::Source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTokenSource {
    User,
    Agent,
    Thinking,
    ToolInput,
    ToolOutput,
}

/// What a repeated run looks like — which is what decides whether there is a fix to suggest.
/// Mirrors `adi_agents::analytics::Shape`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRepeatShape {
    Path,
    Url,
    Literal,
    Block,
    Phrase,
}

/// One place a repeat was sent, addressed the way the transcript view already addresses things: a
/// turn, and a step within it. That is what lets a finding in the rail scroll the feed to the exact
/// tool call that carried it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTokenSite {
    pub turn: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    pub source: AgentTokenSource,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
}

/// How many tokens one source accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTokenSplit {
    pub source: AgentTokenSource,
    pub tokens: usize,
}

/// A run of text the conversation sent more than once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRepeat {
    /// The repeated text, collapsed onto one line and cut for display.
    pub preview: String,
    pub tokens: usize,
    pub count: usize,
    /// Tokens spent on every occurrence after the first.
    pub wasted: usize,
    pub shape: AgentRepeatShape,
    /// What to do about it, when the shape implies something. Empty when it does not — a hint that
    /// fires on every row is one nobody reads, so the server sends none rather than a filler.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub sites: Vec<AgentTokenSite>,
}

/// A group of segments that are nearly, but not exactly, the same — the case an exact-repeat search
/// cannot see, and usually the largest single thing a long run spends its context on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentNearDup {
    pub preview: String,
    pub count: usize,
    /// Tokens in the group's largest member: roughly what one copy costs.
    pub tokens: usize,
    /// Tokens in every member after the first.
    pub wasted: usize,
    pub sites: Vec<AgentTokenSite>,
}

/// `POST /api/agents/run/tokens` — the itemization of one conversation's context. Takes a [`RunRef`].
///
/// Not part of [`AgentPeek`]: the peek is polled every second and this costs a tokenizer pass over
/// the whole transcript, so it is fetched once, when a reader opens the panel that shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTokens {
    pub name: String,
    pub run_id: String,
    /// The encoding the counts are in. Reported because it is a real BPE but not necessarily the
    /// *provider's* — the number is an estimate, and the client says so.
    pub encoding: String,
    pub total: usize,
    pub by_source: Vec<AgentTokenSplit>,
    /// True when only the conversation's recent end was analyzed.
    #[serde(default)]
    pub truncated: bool,
    pub repeats: Vec<AgentRepeat>,
    /// Tokens attributable to exact repetition.
    pub wasted: usize,
    /// Near-identical groups. Counted apart from `wasted`: their overlap with the exact repeats is
    /// real, and summing the two would claim a conversation wasted more than it sent.
    pub near_duplicates: Vec<AgentNearDup>,
}

// ---- the simulator (a run with a person in the model's seat) ------------------------

/// `POST /api/agents/simulate` request — open a run of `name` with a person in the model's seat.
///
/// Always a fresh run: there is no field for a run to continue, deliberately. A simulated turn in
/// the middle of a real conversation would be indistinguishable from the model's own afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulateAgent {
    pub name: String,
    /// What the run opens with — the first user turn, exactly as a real launch's message.
    pub message: String,
}

/// One token of the prompt, as the server's encoder split it.
///
/// The split is done server-side and shipped as data: the ranks are a megabyte and a half, and a
/// browser has no business carrying them to draw a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentToken {
    pub id: u32,
    /// The token's exact text — newlines and leading spaces included, which is most of the point.
    pub text: String,
    /// A chat template's own control token rather than content. Always false today; see
    /// `adi_agents::analytics::PromptToken::special` for why that is honest rather than missing.
    #[serde(default)]
    pub special: bool,
}

/// One labelled stretch of the prompt, as a range of [`AgentSimState::tokens`].
///
/// Ranges rather than nested token lists, so the prompt is still one stream to render and the seams
/// are drawn over it. `to` is exclusive, and the ranges are contiguous and cover the whole prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSimSection {
    /// `instructions`, `where you are`, `what you know`, `your tools`.
    pub label: String,
    pub from: usize,
    pub to: usize,
}

/// One field of a tool call, decoded from the tool's own JSON Schema.
///
/// Decoded on the server, once, rather than in the browser: the schema is the tool's own
/// description of itself, and a second decoder in wasm is a second thing to keep in step with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSimField {
    /// The parameter name, exactly as the tool declares it — it is what goes on the wire.
    pub name: String,
    pub kind: AgentSimFieldKind,
    pub required: bool,
    /// The schema's own description, shown as the field's hint.
    #[serde(default)]
    pub hint: String,
}

/// The control a parameter gets, chosen from its declared type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSimFieldKind {
    /// A single line — a path, a pattern, a name.
    Line,
    /// Something with body to it: a command, a file's contents, a question.
    Text,
    /// `integer` or `number`.
    Number,
    /// An array, written one entry per line.
    List,
    /// `boolean`.
    Flag,
}

/// One tool the run may call, as it is declared to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSimTool {
    pub name: String,
    /// The sentence the model is given about when to reach for it, verbatim.
    pub description: String,
    /// Its parameters, in the order the tool declares them.
    pub fields: Vec<AgentSimField>,
}

/// The whole state of a simulated run — what every simulator endpoint answers with.
///
/// One shape for all four, so a page that stacks a block, ends a turn, or replies gets the new
/// prompt back in the same round trip rather than having to ask again and render something stale in
/// between.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSimState {
    pub name: String,
    pub run_id: String,
    /// The composed system prompt, exactly as the model receives it.
    pub prompt: String,
    /// The same string, split. Concatenated, the token texts are `prompt`.
    pub tokens: Vec<AgentToken>,
    /// Where each part of the prompt begins and ends, in token indices.
    pub sections: Vec<AgentSimSection>,
    /// The encoding the split is in. Reported beside the count, because a token number without its
    /// encoding is a number nobody can check.
    pub encoding: String,
    /// The tools the run may call.
    pub tools: Vec<AgentSimTool>,
    /// The conversation so far — the same turns the chat view renders.
    pub turns: Vec<AgentTurn>,
    /// How the last turn ended: `tool_use`, `end_turn`, or empty before one has.
    #[serde(default)]
    pub stop_reason: String,
    /// Whether the seat is occupied — false once the run has yielded to a person.
    pub running: bool,
}

/// One block a person emitted into the open turn.
///
/// Tagged rather than a struct with optional halves: a block is one thing or the other, and a wire
/// shape that can carry both at once is a shape somebody eventually sends both in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentSimBlock {
    /// Prose. The last one in a turn with no calls is the turn's answer.
    Text { text: String },
    /// A call to one of the agent's tools.
    Call {
        name: String,
        input: serde_json::Value,
    },
}

/// `POST /api/agents/simulate/turn` request — close the open turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulateTurn {
    pub name: String,
    pub run_id: String,
    /// Everything emitted this turn, in the order it was written.
    pub blocks: Vec<AgentSimBlock>,
}

/// What one call returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSimResult {
    pub name: String,
    /// What the model would read next — the tool's output, or its refusal. Both are answers.
    pub output: String,
    /// Whether the tool succeeded. `false` is a result to act on, not a failed request.
    pub ok: bool,
}

/// `POST /api/agents/simulate/turn` response — what the calls returned, and the state after them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSimTurn {
    /// One entry per call, in the order they ran.
    pub results: Vec<AgentSimResult>,
    /// The run after the turn — including the prompt, which has grown by it.
    pub state: AgentSimState,
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
    /// The tool ids to enable on the meta-agent: **every active tool** in the store. The setup
    /// form saves these (unioned with whatever the agent already has), so the one agent that
    /// operates the whole environment always carries the full tool set — including tools
    /// registered after it was created.
    #[serde(default)]
    pub default_bin_tools: Vec<String>,
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
/// into the terminal, then press `key` (a key name). Either part may be empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTermKeys {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub key: String,
}

/// A workspace terminal snapshot: whether its pty session is live and the visible pane text —
/// the workspace twin of `AgentPeek`, polled by the live view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTerm {
    pub id: String,
    pub name: String,
    /// Whether the terminal's pty session is live; `output` is empty when it isn't.
    pub running: bool,
    /// The visible pane text (trailing whitespace trimmed).
    #[serde(default)]
    pub output: String,
    /// A pty session has no external attach command — it is viewed only in the control panel —
    /// so this is always empty. Kept for wire compatibility.
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// CPU and memory of the process tree behind `primary_port`; `None` while the service is
    /// down or when the host could not sample it.
    #[serde(default)]
    pub usage: Option<ProcessUsage>,
}

/// `GET /api/hive` — every Hive service across all projects plus the global front-door hive,
/// each with a live running/stopped flag and, when it is up, what it costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HiveState {
    pub services: Vec<HiveService>,
}

/// One dashboard under `~/.adi/mono/dashboards/<id>/` — a bun-served frontend + backend pair
/// whose UI is authored as loose `.ts` files by agents.
///
/// **One dashboard is one origin** (`docs/fleet.md` §4): both services declare the same
/// `proxy.host`, the frontend owning `/` and the backend `/api`. So [`host`](Self::host) — not a
/// port — is the dashboard's address. The loopback ports remain on the wire because they are what
/// says whether each half is actually *up*, but reaching a dashboard past its front door is what
/// breaks its `/api` calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dashboard {
    /// The directory name, which is also how its hive services are keyed (`<id>/frontend`).
    pub id: String,
    /// The absolute path of that directory. Stated by the server rather than rebuilt client-side
    /// from the id, so anything that has to *point* at a dashboard — the agent embed launching its
    /// chat in it — uses the store's real layout instead of a hardcoded copy of it.
    #[serde(default)]
    pub dir: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The project this dashboard is filed under (its id), or `None` for an unfiled dashboard.
    /// Purely organizational — a dashboard still runs on its own port regardless.
    #[serde(default)]
    pub project: Option<String>,
    /// The hostname both of its services declare (`nosh.adi`) — the dashboard's whole address, and
    /// the only one under which its page's relative `/api` calls route.
    ///
    /// `None` when the dashboard's hive file declares no `proxy.host`, does not parse, or is
    /// missing: it is then running, but has no routable name, and only its loopback ports can
    /// reach it. Optional rather than a blank string so "no name yet" cannot be mistaken for one,
    /// and `#[serde(default)]` so a payload from a server that predates this field still
    /// deserializes — the wasm client and adi-app are versioned apart.
    #[serde(default)]
    pub host: Option<String>,
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
    /// The node this dashboard was **moved** to (its petname here), set by
    /// `POST /api/dashboards/transfer` in `move` mode. Purely a label on the local remains: the
    /// row is archived, so nothing runs here any more, and this says where it went instead of
    /// leaving an operator to guess why a dashboard they were using stopped.
    ///
    /// `None` after a `copy` — both machines run it then, and neither is the one that moved.
    #[serde(default)]
    pub moved_to: Option<String>,
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
    /// The project to file the new dashboard under (its id), or `None` to leave it unfiled.
    #[serde(default)]
    pub project: Option<String>,
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

/// Request body filing a dashboard under a project — `POST /api/dashboards/project`. An empty /
/// absent `project` unfiles it. Returns a fresh [`DashboardsState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDashboardProject {
    pub id: String,
    #[serde(default)]
    pub project: Option<String>,
}

// MARK: moving a dashboard to another machine (`docs/fleet.md` §10)

/// One file of a [`DashboardBundle`].
///
/// The bytes are base64 rather than text because a dashboard is a directory a human fills: an
/// icon, a font, a fixture `.db` are all ordinary things to find in one, and a transfer that
/// silently dropped whatever was not UTF-8 would be a transfer you cannot trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleFile {
    /// Path relative to the dashboard's own directory, always `/`-separated. Never absolute and
    /// never containing `..` — the receiving side re-checks both before it writes anything.
    pub path: String,
    /// The file's bytes, base64 (standard alphabet, padded).
    pub contents: String,
}

/// A dashboard packed up for another machine — the body of `POST /api/dashboards/import`.
///
/// What is **not** in here is the point. The manifest and `.adi/hive.yaml` are omitted and rebuilt
/// on the far side, because both name things that are true only where they were written: the hive
/// file carries an absolute `working_dir`, and its `proxy.host` may already belong to a different
/// dashboard over there. Everything a person or an agent authored travels verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardBundle {
    /// The dashboard's id, carried across so a second transfer **updates** the copy on the node
    /// instead of leaving a duplicate behind. Ids are UUIDs, so this can never collide with an
    /// unrelated dashboard that was already there.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The project it is filed under here. Honoured on the far side only if a project with that
    /// id exists there too; otherwise the copy arrives unfiled, which is what an id that means
    /// nothing on that machine should do.
    #[serde(default)]
    pub project: Option<String>,
    /// The hostname it answers on where it came from — a *preference*, not an instruction. The
    /// receiving machine keeps the label when it is free there and derives a fresh one when it is
    /// not, because two dashboards on one hostname is a routing coin-flip.
    #[serde(default)]
    pub host: Option<String>,
    pub files: Vec<BundleFile>,
}

/// What a transfer does with the copy it leaves behind — `POST /api/dashboards/transfer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    /// Both machines run it afterwards; nothing here changes.
    Copy,
    /// The local dashboard is archived once the node confirms it has the files, so only one copy
    /// is live. Reversible — Restore brings it back — unless
    /// [`delete_local`](TransferDashboard::delete_local) was also asked for.
    Move,
}

/// `POST /api/dashboards/transfer` — send a dashboard to a paired node and, in
/// [`TransferMode::Move`], stand the local copy down.
///
/// The credential is the node's own Basic-auth pair (`docs/fleet.md` §5). It is supplied per
/// transfer and **stored nowhere**: this machine holds only a verifier for the node's password,
/// never the password, and a transfer is not a reason to start keeping one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDashboard {
    pub id: String,
    /// The destination node's petname, as this machine files it.
    pub node: String,
    pub mode: TransferMode,
    /// With [`TransferMode::Move`], also delete the local directory once the node has confirmed
    /// the import. Ignored for a copy.
    #[serde(default)]
    pub delete_local: bool,
    /// The Basic-auth user. Defaults to the one pairing mints (`adi`) when absent.
    #[serde(default)]
    pub username: Option<String>,
    pub password: String,
}

/// What a completed transfer reports back.
///
/// It carries the *whole* local listing as well as the remote row, so the page updates in one
/// round-trip exactly like every other dashboards mutation — a move changes what is archived here,
/// and re-fetching to discover that would leave the row stale in between.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardTransferred {
    /// The node it landed on, by petname.
    pub node: String,
    /// The dashboard as the node now reports it — its id, its host *there*, its ports once the
    /// node's supervisor has leased them.
    pub dashboard: Dashboard,
    /// Where to open it from here: `http://<label>.<node>.n.adi/`, or `None` when the node gave
    /// the copy no routable name.
    #[serde(default)]
    pub url: Option<String>,
    /// Whether the node was also asked to let this machine reach the new dashboard
    /// (`http:<label>`), and said yes. `false` means the transfer worked but the link above will
    /// answer *not authorized* until somebody grants it on the node.
    #[serde(default)]
    pub granted: bool,
    /// The local listing after the transfer — archived here after a move, unchanged after a copy.
    pub dashboards: DashboardsState,
}

// MARK: viewing a fleet's dashboards (`docs/fleet.md` §11)

/// `GET /api/fleet/dashboards` — what every paired node is running, asked node by node.
///
/// One entry per paired node, in the same petname order `/api/fleet` uses, whether or not that
/// node could be reached: a node that is locked, down or refusing is a state to show, not a row
/// to drop. A machine paired with nobody answers an empty list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetDashboards {
    pub nodes: Vec<NodeDashboards>,
}

/// One node's dashboards as this machine may see them — or why it cannot see them.
///
/// The three states are mutually exclusive and each wants a different thing from the operator:
/// `locked` needs the node's password, `error` needs whatever it says, and neither set means the
/// list is real.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDashboards {
    /// The node's petname, as this machine files it.
    pub node: String,
    /// No credential is stored here for this node, so it was never asked. The password is the
    /// human-scoped half of `docs/fleet.md` §5 and is enforced *on the node*; without it there is
    /// nothing to ask with.
    pub locked: bool,
    /// Why the node could not be listed — already phrased for an operator. `None` when it was.
    #[serde(default)]
    pub error: Option<String>,
    /// What the node calls *this* machine in its own registry, matched by key (§2). `None` when
    /// its fleet page could not be read, which is also why a grant would fail.
    #[serde(default)]
    pub me: Option<String>,
    /// Its live dashboards, in the order the node's own panel listed them.
    pub dashboards: Vec<NodeDashboard>,
}

/// One dashboard running on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDashboard {
    /// The dashboard's id on the node — stable, and what its panel keys it by.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The service label to open, from the single host the dashboard declares (§4): `nosh.adi` →
    /// `nosh`. `None` when it declares none, so there is nothing the mesh could route to.
    #[serde(default)]
    pub service: Option<String>,
    /// Whether the node's supervisor has the page's own server up. A dashboard that is down is
    /// still listed — the failure is then the node's to fix, not a row that vanished.
    pub running: bool,
    /// Whether this machine's grants *on the node* already cover it. `false` means opening it has
    /// to ask for `http:<service>` first (`POST /api/fleet/dashboards/allow`), because pairing
    /// hands out only `http:app` (§8).
    pub allowed: bool,
    /// Where to open it from here: `http://<service>.<node>.n.adi/`. Present whenever there is a
    /// name to route to, even while [`allowed`](Self::allowed) is false — the page decides whether
    /// to offer it as a link or as an ask.
    #[serde(default)]
    pub url: Option<String>,
}

/// `POST /api/fleet/dashboards/unlock` — hand this machine a node's password so it can ask that
/// node what it runs.
///
/// Unlike a transfer (which asks per transfer and keeps nothing), this one is **stored**: it goes
/// into the encrypted secrets store, because a rail that re-prompted on every refresh would not be
/// a rail. It is the same bargain `apps/ios` makes with the Keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlockNode {
    pub node: String,
    /// The Basic-auth user. Defaults to the one pairing mints (`adi`) when absent.
    #[serde(default)]
    pub username: Option<String>,
    pub password: String,
}

/// `POST /api/fleet/dashboards/allow` — ask a node to let this machine reach one of its services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeServiceRef {
    pub node: String,
    /// The service label to be granted `http:<service>` on — a dashboard's own label.
    pub service: String,
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

// ---- the shared database -------------------------------------------------------------
//
// Mirrors `adi_db`'s types as plain DTOs. The store crate itself compiles SQLite in and so is
// native-only; these travel to the wasm frontend, which is why they're restated here rather
// than re-exported.

/// One database in the store — the global one, or a project's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbInfoDto {
    /// The project this database belongs to, or `None` for the global database.
    #[serde(default)]
    pub project: Option<String>,
    /// Its absolute path on disk.
    pub path: String,
    /// The size of the main database file in bytes (excluding the `-wal`/`-shm` sidecars).
    pub bytes: u64,
    /// How many tables and views it holds.
    pub tables: usize,
}

/// `GET /api/db` — every database that exists in the store. A scope nothing has written to yet
/// has no file, so it is simply absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbState {
    pub databases: Vec<DbInfoDto>,
}

/// Request body naming a scope — `POST /api/db/tables` and `/schema`. Omitted/blank = global.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DbScope {
    #[serde(default)]
    pub project: Option<String>,
    /// Narrow a schema read to one table. Ignored by `/tables`.
    #[serde(default)]
    pub table: Option<String>,
}

/// One column of a table or view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbColumnDto {
    pub name: String,
    /// The declared type (`TEXT`, `INTEGER`, …) — empty when declared without one.
    pub decl_type: String,
    pub notnull: bool,
    pub pk: bool,
}

/// One table or view, with its shape and current row count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbTableDto {
    pub name: String,
    /// `"table"` or `"view"`.
    pub kind: String,
    pub rows: i64,
    pub columns: Vec<DbColumnDto>,
}

/// `POST /api/db/tables` response — a scope's tables and views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbTablesState {
    #[serde(default)]
    pub project: Option<String>,
    pub tables: Vec<DbTableDto>,
}

/// `POST /api/db/schema` response — the `create` statements for a scope (or one table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbSchema {
    #[serde(default)]
    pub project: Option<String>,
    pub schema: String,
}

/// Request body running SQL — `POST /api/db/query` (read-only) and `/exec` (read-write).
// Not `Eq`: `params` carries `serde_json::Value`, which is `PartialEq` but not `Eq`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DbQuery {
    #[serde(default)]
    pub project: Option<String>,
    pub sql: String,
    /// Values bound to the statement's `?` placeholders, in order.
    ///
    /// Each binds as the JSON type it already is — `42` an integer, `"007"` text — so a caller
    /// never has to encode its types into strings just to get them across the wire.
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
}

/// `POST /api/db/query` response — a result set. Columns sit beside the rows so a table can be
/// laid out without re-deriving the header, and a zero-row result still reports its shape.
// Not `Eq`: cells carry `serde_json::Value`, which is `PartialEq` but not `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DbQueryResult {
    pub columns: Vec<String>,
    /// One entry per row, each aligned to `columns`.
    pub rows: Vec<Vec<serde_json::Value>>,
}

/// `POST /api/db/exec` response — what a statement run for its effect did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbExecResult {
    pub changes: u64,
    pub last_insert_rowid: i64,
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

// ---------------------------------------------------------------- knowledge

/// One knowledge base, with what it holds. Counts come from the store's own status pass, so a
/// base that cannot be opened (a provider this build has never heard of) still lists — with its
/// error where the counts would be, rather than vanishing from the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeBaseDto {
    /// The written id: `global/runbooks`, `project:acme/notes`, `agent:solver/memory`.
    pub id: String,
    /// The isolation level: `global`, `project`, or `agent`.
    pub level: String,
    /// The owner id — a project id or an agent name; absent for a global base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The base's name within its scope.
    pub name: String,
    /// Which backend provider holds it.
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this is an agent's own memory (`agent:<name>/memory`).
    #[serde(default)]
    pub memory: bool,
    pub notes: usize,
    /// How many notes have current vectors …
    pub embedded: usize,
    /// … and how many need embedding or re-embedding.
    pub stale: usize,
    /// Why the counts are missing, when they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// One storage provider a base can be held in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeProviderDto {
    pub name: String,
    pub description: String,
}

/// `GET /api/knowledge` — every base, and what this build can hold one in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KnowledgeState {
    pub bases: Vec<KnowledgeBaseDto>,
    pub providers: Vec<KnowledgeProviderDto>,
    /// The embedding model, once one has been loaded. `None` means nothing has needed it yet —
    /// not that none is available, which is why the page says "not loaded" rather than "none".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One note. `body` is the whole note on a single-note read, and a preview in a list — the
/// page never has to ask which, because a list is never long enough to matter and a read is
/// always deliberate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeNoteDto {
    pub id: String,
    pub base: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Whether its vectors are current — a `false` here is what the page marks as "not embedded".
    #[serde(default)]
    pub embedded: bool,
    #[serde(default)]
    pub chunks: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// `POST /api/knowledge/notes` — the notes in one base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeNotes {
    pub base: String,
    pub notes: Vec<KnowledgeNoteDto>,
}

/// One search result: the note, how well it matched, and which chunk of it did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHitDto {
    #[serde(flatten)]
    pub note: KnowledgeNoteDto,
    pub score: f32,
    #[serde(default)]
    pub chunk: u32,
}

/// `POST /api/knowledge/search` — what was asked, and what came back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeResults {
    pub query: String,
    /// Whether this ranked by meaning (embeddings) or by words (full text).
    pub semantic: bool,
    /// The bases actually searched, so a result page can say what it covered.
    #[serde(default)]
    pub bases: Vec<String>,
    pub hits: Vec<KnowledgeHitDto>,
}

/// `POST /api/knowledge/search` request. An empty `bases` searches everything readable.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KnowledgeSearch {
    pub query: String,
    #[serde(default)]
    pub bases: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// Rank by words instead of by meaning — no model, no wait.
    #[serde(default)]
    pub text: bool,
}

/// A base by id — for status, delete, re-embed, and listing its notes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KnowledgeBaseRef {
    pub base: String,
    /// Listing only: keep the notes carrying every one of these tags.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `POST /api/knowledge/base/create`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewKnowledgeBase {
    pub base: String,
    /// Which provider holds it; omitted means the default (sqlite).
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `POST /api/knowledge/note/add`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewKnowledgeNote {
    pub base: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// One note by id, within its base.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KnowledgeNoteRef {
    pub base: String,
    pub id: String,
}

/// What a write to a note answers with: the note, and whether it came back searchable by
/// meaning. An unembedded note is still stored — the reason travels with the result rather than
/// failing the write, so the page can say so instead of the user finding out at search time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeSaved {
    pub note: KnowledgeNoteDto,
    pub embedded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_error: Option<String>,
}

/// What a re-embed pass did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KnowledgeReembed {
    pub base: String,
    pub scanned: usize,
    pub embedded: usize,
    pub unchanged: usize,
    pub chunks: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

/// One way of turning speech into text, as offered to the panel.
///
/// The list is fixed at compile time — these are the services the server knows how to call —
/// but `ready` is not: an engine whose key is missing is still *listed*, because a picker that
/// hides what it cannot do leaves the user with no way to find out what to configure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceEngineDto {
    /// Stable identifier, and the value `POST /api/voice/transcribe` takes as `?engine=`.
    pub id: String,
    /// What the picker shows.
    pub label: String,
    /// Whether choosing it right now would work. For the remote engines this means a key was
    /// found; the browser engine is always ready, because the page either has the API or does
    /// not and only the page can tell.
    pub ready: bool,
    /// The browser does the recognition itself and no audio leaves the machine. The panel needs
    /// this to know it must *not* record and upload for this engine.
    pub in_browser: bool,
    /// Said in the picker under the label: the model used, or which secret is missing.
    pub detail: String,
}

/// `GET /api/voice` — what the panel can dictate through.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VoiceState {
    pub engines: Vec<VoiceEngineDto>,
    /// Which engine to start on when the user has expressed no preference: the first configured
    /// remote one, else the browser. Only a default — the panel remembers its own choice.
    pub default_engine: String,
}

/// `POST /api/voice/transcribe` — what the audio turned out to say.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    /// The engine that produced it, echoed back so the panel can say where the words came from
    /// even if the choice changed while the clip was in flight.
    pub engine: String,
}
