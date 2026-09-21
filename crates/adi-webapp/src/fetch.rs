//! Thin fetch layer over the `/api/*` endpoints, deserializing into the shared DTOs.

use std::cell::RefCell;
use std::collections::BTreeMap;

use adi_webapp_api::types::{
    Accepted, AgentAttachment, AgentAwaits, AgentGoals, AgentKeys, AgentPeek, AgentRef,
    AgentReviewStarted, AgentRunOverrides, AgentRunResult, AgentRuns, AgentSimBlock, AgentSimState,
    AgentSimTurn, AgentSteps, AgentTokens, AgentsState, AllAgentRuns, AnswerRun, ApiError,
    CloseGoal, Dashboard, DashboardRef, DashboardTransferred, DashboardsState, DbExecResult,
    DbQuery, DbQueryResult, DbSchema, DbScope, DbState, DbTablesState, DiagnosticReport,
    DirListing, EmbeddingBackendRef, EmbeddingBackendsDto, FileContent, FilesRef, FleetDashboards,
    FleetGrantRef, FleetInstructions, FleetJoinRef, FleetNodes, FleetRef, FleetRename, FleetState,
    FsContent, FsCreate, FsListing, FsRef, FsWrite, GoalsOf, Health, HideRun, HiveState,
    IgnoreAwait, InstallMarketplaceApp, KnowledgeBaseRef, KnowledgeNoteDto, KnowledgeNoteRef,
    KnowledgeNotes, KnowledgeReembed, KnowledgeResults, KnowledgeSaved, KnowledgeSearch,
    KnowledgeState, LAUNCHED_BY_HUMAN, LeaseRef, LinkTool, LlmBackendRef, LlmBackendsDto,
    LlmCallDetail, LlmCallRef, LlmCalls, LlmQuery, LlmSummary, MarketplaceDone, MarketplaceState,
    MeshForwardRef, MeshListenRef, MeshPeerRef, MeshPortRef, MeshState, MetaState, NewDashboard,
    NewKnowledgeBase, NewKnowledgeNote, NewProject, NewProjectHook, NewService, NewTask, NewTool,
    NewWorkspace, NodeServiceRef, PortsState, ProjectDetail, ProjectHookLog, ProjectHookRef,
    ProjectHookRunResult, ProjectRef, ProjectRenamed, ProjectsState, QueueMode, ReleaseResponse,
    RenameProject, RenameRun, ReplyToRun, ReserveResponse, RevealedSecret, ReviewRun, RunAgent,
    RunRef, RunSteps, RunSystemAction, RunTool, SaveAgent, SaveEmbeddingBackend,
    SaveEmbeddingSettings, SaveLlmBackend, SaveLlmSettings, SaveTrigger, SecretRef, SecretsState,
    SetAutoTitle, SetDashboardProject, SetGoal, SetOAuthSecret, SetRunLimit, SetSecret,
    SetSharedAssets, SetSystemPower, SharedAssetsMode, SharedAssetsState, SimulateAgent,
    SimulateTurn, StarRun, StartMarketplaceApp, StartMarketplaceService, StartResult,
    StartService, StopResult, SystemStatus, TaskRef, TasksState, TestEmbeddingBackend,
    TestLlmBackend, TestResultDto, ToolRef, ToolRunResult, ToolScript, ToolsState, Transcript,
    TranscriptView, TransferDashboard, TriggerFireResult, TriggerLog, TriggerRef, TriggersState,
    UninstallMarketplaceElement, UnlockNode, UnqueueFromRun, UpdateMarketplaceApp,
    UpdateMarketplaceBundle, UpdateState, UsedPorts, VoiceState, WorkspaceCreateResult,
    WorkspaceRef, WorkspaceTerm, WorkspaceTermKeys, WorkspaceTermRef, WorkspacesRef,
    WorkspacesState, WriteFile, WriteToolScript,
};
use gloo_net::http::{Request, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// This machine's own health and the status LED — never the panel-wide source picker's target
/// (`docs/fleet.md` §14): the socket the LED reports on is this machine's, whatever the panel is
/// pointed at.
pub async fn health() -> Result<Health, String> {
    get_local("/api/health").await
}

pub async fn ports() -> Result<PortsState, String> {
    get("/api/ports").await
}

pub async fn used() -> Result<UsedPorts, String> {
    get("/api/ports/used").await
}

/// The shared-assets CDN setting: whether the webapp bundle is served from `cdn.withadi.dev`
/// instead of this instance's own `dist/`, and the base URL it would use.
pub async fn shared_assets() -> Result<SharedAssetsState, String> {
    get("/api/settings/shared-assets").await
}

pub async fn set_shared_assets(mode: SharedAssetsMode) -> Result<SharedAssetsState, String> {
    post("/api/settings/shared-assets", &SetSharedAssets { mode }).await
}

/// The Meta page's state: the well-known `adi-agent` (if set up), the default system prompt, and
/// the agent form schema. Creating/running it reuses the `save_agent` / `run_agent` endpoints.
pub async fn meta() -> Result<MetaState, String> {
    get("/api/meta").await
}

// Auto-update (`docs/adi-update.md`). All three answer the same shape, so the top bar's
// pill re-renders from whichever call it last made.
//
// Always local, never the panel-wide source picker's target (`docs/fleet.md` §14): the version
// pill names what *this* machine's binary is on, and installing a release runs against this
// machine's own updater whatever the picker is pointed at.

/// What is installed, what was last seen published, and whether an install is in flight.
/// Reads two files on the server; safe to poll.
pub async fn update_state() -> Result<UpdateState, String> {
    get_local("/api/update").await
}

/// Go and ask the release manifest. The one call here that leaves the machine, so the pill
/// makes it only when the server says its record has gone stale.
pub async fn check_update() -> Result<UpdateState, String> {
    post_local("/api/update/check", &()).await
}

/// Install the published release. Answers as soon as the updater is running — it restarts the
/// app on its way through, so the socket this reply came over is expected to drop.
pub async fn run_update() -> Result<UpdateState, String> {
    post_local("/api/update/run", &()).await
}

// The System page (`docs/fleet.md` §14's L3): the same exemption as `/api/update/*` above, for
// the same reason — "restart this machine's own process" and "this machine's own services" can
// only ever mean the box actually serving the page, whatever the panel-wide picker is pointed at.

/// Live status of every managed service, plus how many agent runs are live right now.
pub async fn system_status() -> Result<SystemStatus, String> {
    get_local("/api/system").await
}

/// Run one action a service offered on its own row (an id + a verb, straight off the `args` field
/// `system_status` returned) and get the fresh status back.
pub async fn run_system_action(args: Vec<String>) -> Result<SystemStatus, String> {
    post_local("/api/system/action", &RunSystemAction { args }).await
}

/// The platform-wide switch — `adi-mono enable` / `adi-mono disable`. Answers as soon as the
/// command is handed off; it may restart or stop the very process this reply came from.
pub async fn set_system_power(on: bool) -> Result<Accepted, String> {
    post_local("/api/system/power", &SetSystemPower { on }).await
}

/// Bounce every running service except DNS. Same "answers before it's done" shape as
/// [`set_system_power`], and for the same reason.
pub async fn restart_system() -> Result<Accepted, String> {
    post_local("/api/system/restart", &()).await
}

/// Collect one diagnostic archive now and say where the page can download it from.
pub async fn diagnose_system() -> Result<DiagnosticReport, String> {
    post_local("/api/system/diagnose", &()).await
}

pub async fn reserve(body: &LeaseRef) -> Result<ReserveResponse, String> {
    post("/api/ports/reserve", body).await
}

// LLM backends: the registry of ways to answer a turn. Like mesh below, every endpoint answers
// with the whole fresh registry, so an edit and the view of it are one round-trip — and a save
// that lands while a hold is being written cannot leave the page showing half of each.

pub async fn llm_backends() -> Result<LlmBackendsDto, String> {
    get("/api/llm/backends").await
}

/// By value, not by reference: the page hands this future to `apply_mutation`, which needs a
/// `'static` one, and a borrowed body would tie it to the handler that built it.
pub async fn save_llm_backend(body: SaveLlmBackend) -> Result<LlmBackendsDto, String> {
    post("/api/llm/backends/save", &body).await
}

pub async fn delete_llm_backend(id: String) -> Result<LlmBackendsDto, String> {
    post("/api/llm/backends/delete", &LlmBackendRef { id }).await
}

pub async fn save_llm_settings(body: SaveLlmSettings) -> Result<LlmBackendsDto, String> {
    post("/api/llm/settings", &body).await
}

/// Lift a backend's hold by hand — for the operator who knows the subscription is back before the
/// prober's next sweep, or whose backend cannot be probed at all.
pub async fn release_llm_hold(id: String) -> Result<LlmBackendsDto, String> {
    post("/api/llm/holds/release", &LlmBackendRef { id }).await
}

/// A real, billed request through this backend, right now — the form as it stands, whether or not
/// it has ever been saved.
pub async fn test_llm_backend(draft: SaveLlmBackend) -> Result<TestResultDto, String> {
    post("/api/llm/backends/test", &TestLlmBackend { id: String::new(), draft: Some(draft) }).await
}

pub async fn release(body: &LeaseRef) -> Result<ReleaseResponse, String> {
    post("/api/ports/release", body).await
}

// Embedding backends: the registry `indexer`/`knowledge`/`facts` resolve through. Like LLM
// backends above, every endpoint answers with the whole fresh registry, so an edit and the view of
// it are one round trip.

pub async fn embedding_backends() -> Result<EmbeddingBackendsDto, String> {
    get("/api/embeddings/backends").await
}

pub async fn save_embedding_backend(
    body: SaveEmbeddingBackend,
) -> Result<EmbeddingBackendsDto, String> {
    post("/api/embeddings/backends/save", &body).await
}

pub async fn delete_embedding_backend(id: String) -> Result<EmbeddingBackendsDto, String> {
    post("/api/embeddings/backends/delete", &EmbeddingBackendRef { id }).await
}

pub async fn save_embedding_settings(
    assignments: BTreeMap<String, String>,
) -> Result<EmbeddingBackendsDto, String> {
    post("/api/embeddings/settings", &SaveEmbeddingSettings { assignments }).await
}

/// Embed one short string through this backend, right now — the form as it stands, whether or not
/// it has ever been saved.
pub async fn test_embedding_backend(draft: SaveEmbeddingBackend) -> Result<TestResultDto, String> {
    post(
        "/api/embeddings/backends/test",
        &TestEmbeddingBackend { id: String::new(), draft: Some(draft) },
    )
    .await
}

// Mesh: every endpoint returns the fresh MeshState so the page updates in one round-trip.
//
// Always local, never the panel-wide source picker's target (`docs/fleet.md` §14): the mesh daemon
// a picked node runs is *its own*, reached only by pairing with it in the first place — there is no
// sense in which "point the panel at laptop-b" means "configure laptop-b's mesh from here".

pub async fn mesh() -> Result<MeshState, String> {
    get_local("/api/mesh").await
}

pub async fn mesh_start() -> Result<MeshState, String> {
    post_local("/api/mesh/start", &()).await
}

pub async fn mesh_stop() -> Result<MeshState, String> {
    post_local("/api/mesh/stop", &()).await
}

pub async fn mesh_allow(port: u16) -> Result<MeshState, String> {
    post_local("/api/mesh/allow", &MeshPortRef { port }).await
}

pub async fn mesh_deny(port: u16) -> Result<MeshState, String> {
    post_local("/api/mesh/deny", &MeshPortRef { port }).await
}

pub async fn mesh_allow_peer(peer: String) -> Result<MeshState, String> {
    post_local("/api/mesh/peers/allow", &MeshPeerRef { peer }).await
}

pub async fn mesh_deny_peer(peer: String) -> Result<MeshState, String> {
    post_local("/api/mesh/peers/deny", &MeshPeerRef { peer }).await
}

pub async fn mesh_add_forward(body: MeshForwardRef) -> Result<MeshState, String> {
    post_local("/api/mesh/forwards/add", &body).await
}

pub async fn mesh_remove_forward(listen: u16) -> Result<MeshState, String> {
    post_local("/api/mesh/forwards/remove", &MeshListenRef { listen }).await
}

// Fleet: the paired remote nodes. As with mesh, every endpoint answers with the fresh
// FleetState, so an edit and the view of it are one round-trip.
//
// Always local, never the panel-wide source picker's target (`docs/fleet.md` §14): the registry a
// picked node's own panel would show is *its* fleet, not this machine's, and the whole point of
// pairing, granting and renaming here is to shape what *this* machine can reach.

pub async fn fleet() -> Result<FleetState, String> {
    get_local("/api/fleet").await
}

/// Mint a pairing invite and get it back drawn as a QR. The one fleet call that does *not* answer
/// with a `FleetState`: nothing about the registry has changed — a node appears in it only once
/// somebody spends this.
pub async fn fleet_invite() -> Result<adi_webapp_api::types::FleetInvite, String> {
    post_local("/api/fleet/invite", &()).await
}

/// Spend an invite minted on another machine. The slowest call on this page by far — it dials that
/// machine and waits for the handshake — and the only one that answers with a credential: the
/// password inside is the single copy either side will ever hold, so what shows it is what has to
/// let it go.
pub async fn fleet_join(token: String) -> Result<adi_webapp_api::types::FleetJoined, String> {
    post_local("/api/fleet/join", &FleetJoinRef { token }).await
}

pub async fn fleet_rename(petname: String, to: String) -> Result<FleetState, String> {
    post_local("/api/fleet/rename", &FleetRename { petname, to }).await
}

pub async fn fleet_unpair(petname: String) -> Result<FleetState, String> {
    post_local("/api/fleet/unpair", &FleetRef { petname }).await
}

pub async fn fleet_grant(petname: String, grant: String) -> Result<FleetState, String> {
    post_local("/api/fleet/grants/add", &FleetGrantRef { petname, grant }).await
}

pub async fn fleet_revoke(petname: String, grant: String) -> Result<FleetState, String> {
    post_local(
        "/api/fleet/grants/remove",
        &FleetGrantRef { petname, grant },
    )
    .await
}

/// Set or clear a node's standing agent instructions (ADI-MONO-15). An empty `instructions`
/// clears it — see `adi_webapp_api::handlers::fleet::fleet_instructions`.
pub async fn fleet_instructions(
    petname: String,
    instructions: String,
) -> Result<FleetState, String> {
    post_local(
        "/api/fleet/instructions",
        &FleetInstructions {
            petname,
            instructions,
        },
    )
    .await
}

pub async fn fleet_accept_nickname(petname: String) -> Result<FleetState, String> {
    post_local("/api/fleet/nickname/accept", &FleetRef { petname }).await
}

pub async fn fleet_dismiss_nickname(petname: String) -> Result<FleetState, String> {
    post_local("/api/fleet/nickname/dismiss", &FleetRef { petname }).await
}

// The fleet's dashboards: what each paired node runs, asked of that node's own control panel over
// the mesh. Every one of these leaves the machine, so they are slower than the calls above — the
// caller should show the rail as loading rather than assume a local round-trip. Each answers with
// the whole fresh listing, the same one-round-trip contract the rest of `/api/fleet` keeps.

pub async fn fleet_dashboards() -> Result<FleetDashboards, String> {
    get_local("/api/fleet/dashboards").await
}

/// Which paired nodes this machine holds a password for — the cheap read behind the sessions
/// rail's node menu (`docs/fleet.md` §13) and the panel-wide source picker's own list (§14). Local:
/// unlike the listing above it asks no node anything, so it is polled with the rest of the page
/// rather than on a click — and, per §14, it is what the picker offers, so it would be circular for
/// it to follow the picker's own pointer.
pub async fn fleet_nodes() -> Result<FleetNodes, String> {
    get_local("/api/fleet/nodes").await
}

/// Give this machine a node's password, so that node's dashboards can be listed. Checked against
/// the node before it is stored, so a rejected password comes back as an error here rather than as
/// a broken row later.
pub async fn unlock_node(node: String, password: String) -> Result<FleetDashboards, String> {
    post_local(
        "/api/fleet/dashboards/unlock",
        &UnlockNode {
            node,
            username: None,
            password,
        },
    )
    .await
}

/// Drop a node's stored password. Nothing on the node changes; this machine just stops asking.
pub async fn forget_node(petname: String) -> Result<FleetDashboards, String> {
    post_local("/api/fleet/dashboards/forget", &FleetRef { petname }).await
}

/// Ask a node to let this machine reach one of its services (`http:<service>`), so a listed
/// dashboard becomes a link that opens rather than one that refuses.
pub async fn allow_node_service(node: String, service: String) -> Result<FleetDashboards, String> {
    post_local(
        "/api/fleet/dashboards/allow",
        &NodeServiceRef { node, service },
    )
    .await
}

// Projects: every endpoint returns the fresh ProjectsState so the page updates in one round-trip.

pub async fn projects() -> Result<ProjectsState, String> {
    get("/api/projects").await
}

pub async fn create_project(body: NewProject) -> Result<ProjectsState, String> {
    post("/api/projects/create", &body).await
}

pub async fn archive_project(id: String) -> Result<ProjectsState, String> {
    post("/api/projects/archive", &ProjectRef { id }).await
}

pub async fn unarchive_project(id: String) -> Result<ProjectsState, String> {
    post("/api/projects/unarchive", &ProjectRef { id }).await
}

pub async fn project_detail(id: &str) -> Result<ProjectDetail, String> {
    get(&format!("/api/projects/{id}")).await
}

pub async fn remove_project(id: String) -> Result<ProjectsState, String> {
    post("/api/projects/remove", &ProjectRef { id }).await
}

/// Give a project a new id. The odd one out here: it answers with a receipt of what followed the
/// project into its new id — which the page has to *say*, since renaming a slug quietly moves
/// secrets and databases — and carries the fresh list inside it.
pub async fn rename_project(id: String, new_id: String) -> Result<ProjectRenamed, String> {
    post("/api/projects/rename", &RenameProject { id, new_id }).await
}

pub async fn tasks() -> Result<TasksState, String> {
    get("/api/tasks").await
}

pub async fn create_task(body: NewTask) -> Result<TasksState, String> {
    post("/api/tasks/create", &body).await
}

/// Archive a task and its open descendants — archiving a parent from the UI takes the whole
/// subtree off the plate, rather than leaving orphaned subtasks re-rooted in the live list.
pub async fn archive_task(id: String) -> Result<TasksState, String> {
    post("/api/tasks/archive", &TaskRef { id, cascade: true }).await
}

pub async fn reopen_task(id: String) -> Result<TasksState, String> {
    post("/api/tasks/reopen", &TaskRef { id, cascade: false }).await
}

/// Permanently delete a task; its direct children reparent to its parent. Irreversible.
pub async fn delete_task(id: String) -> Result<TasksState, String> {
    post("/api/tasks/delete", &TaskRef { id, cascade: false }).await
}

// Tools: every mutation returns the fresh ToolsState so the page updates in one round-trip.

pub async fn tools() -> Result<ToolsState, String> {
    get("/api/tools").await
}

pub async fn create_tool(body: NewTool) -> Result<ToolsState, String> {
    post("/api/tools/create", &body).await
}

pub async fn link_tool(body: LinkTool) -> Result<ToolsState, String> {
    post("/api/tools/link", &body).await
}

pub async fn archive_tool(id: String) -> Result<ToolsState, String> {
    post("/api/tools/archive", &ToolRef { id }).await
}

pub async fn unarchive_tool(id: String) -> Result<ToolsState, String> {
    post("/api/tools/unarchive", &ToolRef { id }).await
}

/// Permanently delete a tool; a linked target file is never touched. Irreversible.
pub async fn remove_tool(id: String) -> Result<ToolsState, String> {
    post("/api/tools/remove", &ToolRef { id }).await
}

pub async fn read_tool_script(id: String) -> Result<ToolScript, String> {
    post("/api/tools/script/read", &ToolRef { id }).await
}

pub async fn write_tool_script(id: String, content: String) -> Result<ToolScript, String> {
    post("/api/tools/script/write", &WriteToolScript { id, content }).await
}

/// Run a tool once and capture its output, plus the fresh tools state.
pub async fn run_tool(id: String, args: Vec<String>) -> Result<ToolRunResult, String> {
    post("/api/tools/run", &RunTool { id, args }).await
}

// Secrets: list/set/remove return the fresh SecretsState (metadata only). `reveal` is the one
// call that returns a value — kept separate so a value only ever crosses the wire on demand.

pub async fn secrets() -> Result<SecretsState, String> {
    get("/api/secrets").await
}

pub async fn set_secret(body: SetSecret) -> Result<SecretsState, String> {
    post("/api/secrets/set", &body).await
}

pub async fn remove_secret(project: Option<String>, name: String) -> Result<SecretsState, String> {
    post("/api/secrets/remove", &SecretRef { project, name }).await
}

pub async fn reveal_secret(
    project: Option<String>,
    name: String,
) -> Result<RevealedSecret, String> {
    post("/api/secrets/reveal", &SecretRef { project, name }).await
}

/// Store a secret whose value came from an OAuth flow (access token + refresh token + metadata).
pub async fn set_oauth_secret(body: SetOAuthSecret) -> Result<SecretsState, String> {
    post("/api/secrets/set-oauth", &body).await
}

/// Renew an OAuth secret's access token from its stored refresh token — done server-side, so the
/// refresh token never reaches the browser.
pub async fn refresh_secret(project: Option<String>, name: String) -> Result<SecretsState, String> {
    post("/api/secrets/refresh", &SecretRef { project, name }).await
}

// The shared SQLite store. `query` and `exec` are separate endpoints because the server holds a
// read-only connection for one and a read-write connection for the other — browsing can't write.

pub async fn db() -> Result<DbState, String> {
    get("/api/db").await
}

pub async fn db_tables(project: Option<String>) -> Result<DbTablesState, String> {
    post(
        "/api/db/tables",
        &DbScope {
            project,
            table: None,
        },
    )
    .await
}

pub async fn db_schema(project: Option<String>, table: Option<String>) -> Result<DbSchema, String> {
    post("/api/db/schema", &DbScope { project, table }).await
}

/// Run a read-only statement and get its rows back.
pub async fn db_query(project: Option<String>, sql: String) -> Result<DbQueryResult, String> {
    post(
        "/api/db/query",
        &DbQuery {
            project,
            sql,
            params: Vec::new(),
        },
    )
    .await
}

/// Run a statement for its effect — DDL, or insert/update/delete.
pub async fn db_exec(project: Option<String>, sql: String) -> Result<DbExecResult, String> {
    post(
        "/api/db/exec",
        &DbQuery {
            project,
            sql,
            params: Vec::new(),
        },
    )
    .await
}

// The LLM gateway's journal — every model API call this machine made through `llm.adi`. Reads
// only: the record is the gateway's to write, and the panel has no endpoint that could edit it.

/// The traffic over a window, from three angles, plus what the filters may offer.
pub async fn llm_summary(query: &LlmQuery) -> Result<LlmSummary, String> {
    post("/api/llm/summary", query).await
}

/// The calls themselves, newest first — everything but the bodies.
pub async fn llm_calls(query: &LlmQuery) -> Result<LlmCalls, String> {
    post("/api/llm/calls", query).await
}

/// One call, analyzed: the prompt as blocks, the answer reassembled, both header sets.
pub async fn llm_call(id: i64) -> Result<LlmCallDetail, String> {
    post("/api/llm/call", &LlmCallRef { id }).await
}

// Agents: every endpoint returns the fresh AgentsState so the page updates in one round-trip.

pub async fn agents() -> Result<AgentsState, String> {
    get("/api/agents").await
}

/// A paired node's own `/api/agents` — one entry in the sessions rail's per-source merge
/// (`docs/fleet.md` §13, multi-select). Local is never asked through here: [`agents`] above is kept
/// fresh already, on the schedule every other page that reads it relies on, and a second fetch of the
/// same answer under a different name would be two clocks for one fact.
pub async fn agents_on(node: &str) -> Result<AgentsState, String> {
    get_on(Some(node), "/api/agents").await
}

pub async fn save_agent(body: SaveAgent) -> Result<AgentsState, String> {
    post("/api/agents/save", &body).await
}

pub async fn delete_agent(name: String) -> Result<AgentsState, String> {
    post("/api/agents/delete", &AgentRef { name }).await
}

/// Launch a run. `working_dir` is the composer's optional "run here" — blank means "run this agent
/// as defined", so it starts where its manifest and its project say. `overrides` is the rest of the
/// same panel: the agent's own settings this one run replaces, `None` for a launch that changes
/// nothing. `force` launches past a full concurrency limit — what the "Run anyway" affordance sends.
///
/// `start_at` is the same panel's "start on": the backend this conversation begins on, `None` for
/// the first one the agent lists. It rotates the agent's list rather than cutting it, so a run begun
/// on a second choice still has the rest behind it.
///
/// `node` is which source the composer is pointed at (`docs/fleet.md` §13) — `None` for this
/// machine, routed the same way every other agent-scoped call in this file is.
pub async fn run_agent(
    node: Option<&str>,
    name: String,
    message: String,
    working_dir: Option<String>,
    overrides: Option<AgentRunOverrides>,
    start_at: Option<String>,
    force: bool,
    attachments: Vec<String>,
) -> Result<AgentRunResult, String> {
    post_on(
        node,
        "/api/agents/run",
        &RunAgent {
            name,
            message,
            working_dir,
            overrides,
            start_at,
            // Pinning a run to one backend with nothing behind it is a deliberate "ask *this*
            // model", which the CLI and the API offer and this composer does not: a chat started
            // from the panel should keep the rest of its chain to fall back on.
            only: None,
            force,
            attachments,
            // The composer launches what a person typed; a pre-run is something a launcher that
            // already knows the agent's first move sends (the CLI's `--pre-run`, a filer's API
            // call). Nothing in this form offers one, so it sends none.
            pre_run: Vec::new(),
            // Somebody pressed Send. Stated rather than left to the endpoint's default, because
            // this is the one caller that can be sure of it — and it is what puts the session in
            // the rail's "started by me".
            launched_by: Some(LAUNCHED_BY_HUMAN.to_string()),
        },
    )
    .await
}

/// Set how many agent runs may be live at once: the global cap, or one project's own when
/// `project` names one (`0` lifts / clears).
pub async fn set_run_limit(
    max_concurrent_runs: u32,
    project: Option<String>,
) -> Result<AgentsState, String> {
    post(
        "/api/agents/limit",
        &SetRunLimit {
            max_concurrent_runs,
            project,
        },
    )
    .await
}

/// Turn the auto-title guesser on or off — see `SetAutoTitle`.
pub async fn set_auto_title(enabled: bool) -> Result<AgentsState, String> {
    post("/api/agents/auto-title", &SetAutoTitle { enabled }).await
}

pub async fn stop_agent(name: String) -> Result<AgentsState, String> {
    post("/api/agents/stop", &AgentRef { name }).await
}

/// A headless agent's run history, newest first — the *open conversation's* own source
/// (`docs/fleet.md` §13), not necessarily this machine's.
pub async fn agent_runs(node: Option<&str>, name: String) -> Result<AgentRuns, String> {
    post_on(node, "/api/agents/runs", &AgentRef { name }).await
}

/// Every agent's run history in one call — the data behind the cross-agent "All chats" index,
/// **whole**: a hidden run comes back like any other, which is what a workbench is for.
///
/// `limit` asks for only the newest N sessions across every agent; `None`, which is what Analytics
/// and the Agents index ask for, is the whole history. The answer carries `total` either way, so a
/// paged caller knows whether there is more behind it.
pub async fn all_agent_runs(limit: Option<usize>) -> Result<AllAgentRuns, String> {
    get(&all_runs_path(limit)).await
}

/// The `/api/agents/runs/all` request for a given page size, whole history — one function,
/// because the live channel watches this path by string and a subscription that spelled it
/// differently would be a second topic answering the same question.
pub fn all_runs_path(limit: Option<usize>) -> String {
    match limit {
        Some(n) => format!("/api/agents/runs/all?limit={n}"),
        None => "/api/agents/runs/all".to_string(),
    }
}

/// The sessions rail's own page: [`all_agent_runs`], narrowed server-side to what the main list may
/// draw — a hidden run stays out unless it is asking a question nobody has answered
/// (`docs/sessions.md`).
pub async fn all_agent_runs_visible(limit: Option<usize>) -> Result<AllAgentRuns, String> {
    get(&all_visible_runs_path(limit)).await
}

/// [`all_agent_runs_visible`], for one paired node's own sessions — the sessions rail's per-source
/// merge (`docs/fleet.md` §13, multi-select).
pub async fn all_agent_runs_visible_on(
    node: &str,
    limit: Option<usize>,
) -> Result<AllAgentRuns, String> {
    get_on(Some(node), &all_visible_runs_path(limit)).await
}

/// The rail's own page of the index — see [`all_agent_runs_visible`]. Its own path function, for
/// the same reason [`all_runs_path`] has one: the live channel keys a subscription by this string.
pub fn all_visible_runs_path(limit: Option<usize>) -> String {
    match limit {
        Some(n) => format!("/api/agents/runs/all?limit={n}&hidden=false"),
        None => "/api/agents/runs/all?hidden=false".to_string(),
    }
}

/// The rail's **Hidden** band, unpaged — every run put away with Hide, minus one already shown
/// live with a question on it. Fetched only while the band is open, never on the rail's ordinary
/// poll (`docs/sessions.md`).
pub async fn hidden_runs() -> Result<AllAgentRuns, String> {
    get(hidden_runs_path()).await
}

/// [`hidden_runs`], for one paired node's own sessions.
pub async fn hidden_runs_on(node: &str) -> Result<AllAgentRuns, String> {
    get_on(Some(node), hidden_runs_path()).await
}

/// The Hidden band's own path — never paged, since that band draws every hidden run it has rather
/// than a screenful of them. A function rather than a bare constant for the same reason
/// [`all_runs_path`] is one: the live channel keys a subscription by this string.
pub fn hidden_runs_path() -> &'static str {
    "/api/agents/runs/all?hidden=true"
}

/// A snapshot of one specific run's log (plus the conversation transcript, for harness runs), from
/// the source that run actually lives on (`docs/fleet.md` §13) — `None` for this machine.
///
/// `view` is how much of the transcript to bring back ([`TranscriptView`]): the chat asks for its
/// page, folded, and everything else asks for the whole thing exactly as it always did.
pub async fn peek_run(
    node: Option<&str>,
    name: String,
    run_id: String,
    view: TranscriptView,
) -> Result<AgentPeek, String> {
    post_on(node, "/api/agents/run/peek", &RunRef { name, run_id, view }).await
}

/// The calls behind one folded run — what a reader's click asks for.
///
/// One read per run opened, not per call: `from..to` is the range the run's header named. The
/// answer is final for a settled turn, which is why the caller caches it and never asks twice.
pub async fn run_steps(
    node: Option<&str>,
    name: String,
    run_id: String,
    turn: usize,
    from: usize,
    to: usize,
) -> Result<AgentSteps, String> {
    post_on(
        node,
        "/api/agents/run/steps",
        &RunSteps {
            name,
            run_id,
            turn,
            from,
            to,
        },
    )
    .await
}

/// The itemization of one conversation's context: how its tokens split by source, and which runs of
/// text were sent more than once. Asked for once, when the reader opens the panel — it re-tokenizes
/// the transcript and has no business on the one-second poll.
pub async fn run_tokens(
    node: Option<&str>,
    name: String,
    run_id: String,
) -> Result<AgentTokens, String> {
    post_on(
        node,
        "/api/agents/run/tokens",
        &RunRef {
            name,
            run_id,
            ..RunRef::default()
        },
    )
    .await
}

/// Open a run of an agent with a person in the model's seat. Always a fresh run.
pub async fn simulate_agent(name: String, message: String) -> Result<AgentSimState, String> {
    post("/api/agents/simulate", &SimulateAgent { name, message }).await
}

/// The simulated run as the model sees it: the composed prompt, its split, its tools, its turns.
pub async fn simulate_prompt(name: String, run_id: String) -> Result<AgentSimState, String> {
    post(
        "/api/agents/simulate/prompt",
        &RunRef {
            name,
            run_id,
            ..RunRef::default()
        },
    )
    .await
}

/// Close the open turn: every call in it runs, for real, in the agent's own environment. What comes
/// back is what the calls returned *and* the run after them, so the prompt on screen is never a turn
/// behind what was just done.
pub async fn simulate_turn(
    name: String,
    run_id: String,
    blocks: Vec<AgentSimBlock>,
) -> Result<AgentSimTurn, String> {
    post(
        "/api/agents/simulate/turn",
        &SimulateTurn {
            name,
            run_id,
            blocks,
        },
    )
    .await
}

/// Answer a yielded simulated run as yourself.
pub async fn simulate_reply(
    name: String,
    run_id: String,
    message: String,
) -> Result<AgentSimState, String> {
    post(
        "/api/agents/simulate/reply",
        &ReplyToRun {
            name,
            run_id,
            message,
            // A simulated turn is a person in the model's seat, typing into a form. There is no
            // composer there and so nothing to attach.
            attachments: Vec::new(),
            // Nor is there a running turn to overtake — a simulated conversation only ever answers
            // between turns.
            mode: QueueMode::Regular,
            // The simulator draws the whole conversation it is building; there is no page of it.
            view: TranscriptView::default(),
        },
    )
    .await
}

/// Hand one conversation to the root agent and ask how the workflow should have gone. Writes the
/// dossier server-side and launches the reviewer on it; what comes back is where to watch, not the
/// review itself — the review is a conversation, and it is only starting.
///
/// Runs on the conversation's own source (`node`): the reviewer it launches is a conversation on
/// that same machine, so the caller keeps `node` unchanged when it goes to open it.
pub async fn review_run(
    node: Option<&str>,
    name: String,
    run_id: String,
) -> Result<AgentReviewStarted, String> {
    post_on(
        node,
        "/api/agents/run/review",
        &ReviewRun {
            name,
            run_id,
            reviewer: String::new(),
        },
    )
    .await
}

/// Say something into one of a harness agent's conversations: it starts the next turn, or queues
/// behind the answer still in flight — at the back of the line (`mode: Regular`) or, for
/// `harness:adi`, overtaking it (`mode: Asap`; see [`QueueMode`]). Returns a fresh snapshot with
/// the updated transcript (including the streaming answer and anything queued).
pub async fn reply_to_run(
    node: Option<&str>,
    name: String,
    run_id: String,
    message: String,
    attachments: Vec<String>,
    mode: QueueMode,
    view: TranscriptView,
) -> Result<AgentPeek, String> {
    post_on(
        node,
        "/api/agents/run/reply",
        &ReplyToRun {
            name,
            run_id,
            message,
            attachments,
            mode,
            view,
        },
    )
    .await
}

/// Settle the question a conversation is waiting on, with one reply per question in the order they
/// were asked. `ask` names the ask so a card left open in another tab cannot answer the question
/// that has since replaced it — a stale one comes back 404, which is the useful answer.
pub async fn answer_run(
    node: Option<&str>,
    name: String,
    run_id: String,
    ask: String,
    replies: Vec<String>,
    view: TranscriptView,
) -> Result<AgentPeek, String> {
    post_on(
        node,
        "/api/agents/run/answer",
        &AnswerRun {
            name,
            run_id,
            ask: Some(ask),
            replies,
            view,
        },
    )
    .await
}

/// One conversation's goals, open and closed alike — what it is for, and what it already settled.
pub async fn agent_goals(
    node: Option<&str>,
    name: String,
    run_id: String,
) -> Result<AgentGoals, String> {
    post_on(node, "/api/agents/goals", &GoalsOf { name, run_id }).await
}

/// Write a goal onto a conversation, or reword one that is open (`goal` names which).
///
/// A goal set here is always recorded as set by a person: a run setting its own goes through the
/// CLI from inside its turn, and the two are worth telling apart afterward.
pub async fn set_agent_goal(
    node: Option<&str>,
    name: String,
    run_id: String,
    text: String,
    goal: Option<String>,
) -> Result<AgentGoals, String> {
    post_on(
        node,
        "/api/agents/goal/set",
        &SetGoal {
            name,
            run_id,
            text,
            goal,
        },
    )
    .await
}

/// Close a goal — `met` as done, anything else as given up on, with the evidence or the reason.
///
/// A goal somebody already closed comes back with the ending that happened rather than an error;
/// only an id naming no goal at all is a 404.
pub async fn close_agent_goal(
    node: Option<&str>,
    goal: String,
    as_: String,
    note: String,
) -> Result<AgentGoals, String> {
    post_on(
        node,
        "/api/agents/goal/close",
        &CloseGoal { goal, as_, note },
    )
    .await
}

/// Stop waiting on one of a conversation's registered wakes, returning the ones it still holds.
///
/// The only write over the await store from here: a run registers its own from inside a turn, and
/// what a person needs is the other direction — a wake that is never coming, taken off a
/// conversation that would otherwise sit open until it expires.
pub async fn ignore_agent_await(
    node: Option<&str>,
    name: String,
    run_id: String,
    id: String,
) -> Result<AgentAwaits, String> {
    post_on(
        node,
        "/api/agents/await/ignore",
        &IgnoreAwait { name, run_id, id },
    )
    .await
}

/// Drop the message at `index` from a conversation's queue, returning the fresh snapshot.
pub async fn unqueue_from_run(
    node: Option<&str>,
    name: String,
    run_id: String,
    index: usize,
    view: TranscriptView,
) -> Result<AgentPeek, String> {
    post_on(
        node,
        "/api/agents/run/unqueue",
        &UnqueueFromRun {
            name,
            run_id,
            index,
            view,
        },
    )
    .await
}

/// Stop one specific run, returning the fresh run history — from `node`, the row's own origin
/// (`docs/fleet.md` §13), not necessarily this machine.
pub async fn stop_run(
    node: Option<&str>,
    name: String,
    run_id: String,
) -> Result<AgentRuns, String> {
    post_on(
        node,
        "/api/agents/run/stop",
        &RunRef {
            name,
            run_id,
            ..RunRef::default()
        },
    )
    .await
}

/// Delete one run outright — for a harness agent, the whole conversation — returning the fresh run
/// history without it. Routed to the row's own origin, like every other row action.
pub async fn delete_run(
    node: Option<&str>,
    name: String,
    run_id: String,
) -> Result<AgentRuns, String> {
    post_on(
        node,
        "/api/agents/run/delete",
        &RunRef {
            name,
            run_id,
            ..RunRef::default()
        },
    )
    .await
}

/// Hide one session from the chat rail, or bring it back (`hidden: false`). Nothing is deleted and
/// nothing is stopped — the fresh run history still carries the run, now flagged `hidden`. Routed to
/// the row's own origin, like every other row action.
pub async fn hide_run(
    node: Option<&str>,
    name: String,
    run_id: String,
    hidden: bool,
) -> Result<AgentRuns, String> {
    post_on(
        node,
        "/api/agents/run/hide",
        &HideRun {
            name,
            run_id,
            hidden,
        },
    )
    .await
}

/// Star one conversation, or unstar it (`starred: false`), returning the fresh run history with the
/// flag on it. Nothing is deleted and nothing is stopped — but a starred conversation is also the
/// one the per-agent cap will not sweep, so this is how a chat is kept past the fifty newest. Routed
/// to the row's own origin, like every other row action.
pub async fn star_run(
    node: Option<&str>,
    name: String,
    run_id: String,
    starred: bool,
) -> Result<AgentRuns, String> {
    post_on(
        node,
        "/api/agents/run/star",
        &StarRun {
            name,
            run_id,
            starred,
        },
    )
    .await
}

/// Give one conversation a name of its own, or (`title: ""`) clear it back to the title derived
/// from what it was opened with, returning the fresh run history with the new title on it. Routed
/// to the row's own origin, like every other row action.
pub async fn rename_run(
    node: Option<&str>,
    name: String,
    run_id: String,
    title: String,
) -> Result<AgentRuns, String> {
    post_on(
        node,
        "/api/agents/run/rename",
        &RenameRun {
            name,
            run_id,
            title,
        },
    )
    .await
}

pub async fn peek_agent(node: Option<&str>, name: String) -> Result<AgentPeek, String> {
    post_on(node, "/api/agents/peek", &AgentRef { name }).await
}

pub async fn send_agent_keys(
    node: Option<&str>,
    name: String,
    text: String,
    key: String,
) -> Result<AgentPeek, String> {
    post_on(
        node,
        "/api/agents/send-keys",
        &AgentKeys { name, text, key },
    )
    .await
}

// Triggers: every endpoint returns the fresh TriggersState so the page updates in one round-trip.

pub async fn triggers() -> Result<TriggersState, String> {
    get("/api/triggers").await
}

pub async fn save_trigger(body: SaveTrigger) -> Result<TriggersState, String> {
    post("/api/triggers/save", &body).await
}

pub async fn delete_trigger(name: String) -> Result<TriggersState, String> {
    post("/api/triggers/delete", &TriggerRef { name }).await
}

pub async fn fire_trigger(name: String) -> Result<TriggerFireResult, String> {
    post("/api/triggers/fire", &TriggerRef { name }).await
}

/// Replace a supervised background trigger's process with a fresh one, leaving its definition
/// alone.
pub async fn restart_trigger(name: String) -> Result<TriggerFireResult, String> {
    post("/api/triggers/restart", &TriggerRef { name }).await
}

pub async fn trigger_log(name: String) -> Result<TriggerLog, String> {
    post("/api/triggers/log", &TriggerRef { name }).await
}

pub async fn dashboards() -> Result<DashboardsState, String> {
    get("/api/dashboards").await
}

// The apps marketplace (docs/marketplace.md). The listing reads the store; sync is the only
// call that leaves the machine, which is why it is a button and not a poll.

/// Sources and cached entries, with where each app stands on this machine.
pub async fn marketplace() -> Result<MarketplaceState, String> {
    get("/api/marketplace").await
}

/// Fetch every source's manifest. One line per source comes back in the message.
pub async fn sync_marketplace() -> Result<MarketplaceDone, String> {
    post("/api/marketplace/sync", &()).await
}

/// Install a bundle, or one `<kind>/<name>` element of it: clone its repository at the commit the
/// manifest pins, and land the selection. `name`/`start` matter only for a legacy
/// single-dashboard bundle; a general bundle's elements always land under their own published
/// names.
pub async fn install_marketplace_app(
    marketplace: String,
    slug: String,
    element: Option<String>,
    name: String,
    start: bool,
    project: Option<String>,
    new_project: Option<String>,
) -> Result<MarketplaceDone, String> {
    post(
        "/api/marketplace/install",
        &InstallMarketplaceApp {
            marketplace,
            slug,
            element,
            project,
            new_project,
            name,
            start,
        },
    )
    .await
}

/// Uninstall one installed element of a bundle by its own kind's ordinary path. Every sibling is
/// left untouched.
pub async fn uninstall_marketplace_element(
    marketplace: String,
    slug: String,
    element: String,
    project: Option<String>,
) -> Result<MarketplaceDone, String> {
    post(
        "/api/marketplace/uninstall",
        &UninstallMarketplaceElement {
            marketplace,
            slug,
            element,
            project,
        },
    )
    .await
}

/// Start an installed copy — the act that runs it.
pub async fn start_marketplace_app(id: String) -> Result<MarketplaceDone, String> {
    post("/api/marketplace/start", &StartMarketplaceApp { id }).await
}

/// Copy a parked hive service element's block into the live hive.yaml it belongs in. Idempotent —
/// safe to press again on one already started.
pub async fn start_marketplace_service(
    marketplace: String,
    slug: String,
    name: String,
    project: Option<String>,
) -> Result<MarketplaceDone, String> {
    post(
        "/api/marketplace/start-service",
        &StartMarketplaceService {
            marketplace,
            slug,
            name,
            project,
        },
    )
    .await
}

/// Move an installed copy onto the commit its marketplace now pins. `force` resets onto the pin
/// and loses local work; without it an update that cannot fast-forward is refused.
pub async fn update_marketplace_app(id: String, force: bool) -> Result<MarketplaceDone, String> {
    post(
        "/api/marketplace/update",
        &UpdateMarketplaceApp { id, force },
    )
    .await
}

/// Fast-forward a general bundle's own internal clone onto its manifest's current pin, and
/// re-apply every ledgered element, per element — a drifted one is left alone and reported unless
/// `force` names it.
pub async fn update_marketplace_bundle(
    marketplace: String,
    slug: String,
    force: Vec<String>,
    project: Option<String>,
) -> Result<MarketplaceDone, String> {
    post(
        "/api/marketplace/bundle/update",
        &UpdateMarketplaceBundle {
            marketplace,
            slug,
            force,
            project,
        },
    )
    .await
}

/// Scaffold a new dashboard; the supervisor starts it within a few seconds.
pub async fn create_dashboard(body: NewDashboard) -> Result<Dashboard, String> {
    post("/api/dashboards/create", &body).await
}

/// Archive a dashboard: park its hive file so the supervisor stops both bun services, and hide
/// the row. Returns the fresh state so the page updates in one round-trip.
pub async fn archive_dashboard(id: String) -> Result<DashboardsState, String> {
    post("/api/dashboards/archive", &DashboardRef { id }).await
}

/// Restore an archived dashboard: the supervisor restarts both services on the same leased ports.
pub async fn unarchive_dashboard(id: String) -> Result<DashboardsState, String> {
    post("/api/dashboards/unarchive", &DashboardRef { id }).await
}

/// Permanently delete an archived dashboard's directory (all its files). Irreversible; the backend
/// refuses unless the dashboard is archived first.
pub async fn delete_dashboard(id: String) -> Result<DashboardsState, String> {
    post("/api/dashboards/delete", &DashboardRef { id }).await
}

/// Send a dashboard to a paired node and run it there — a copy, or a move that archives the local
/// one (`docs/fleet.md` §10). The password is the node's own; it is used for this one request and
/// stored nowhere, here or on the server.
///
/// Slower than every other call on this page: it uploads the dashboard's files over the mesh, so
/// the caller should show the form as busy rather than assume a round-trip.
pub async fn transfer_dashboard(body: TransferDashboard) -> Result<DashboardTransferred, String> {
    post("/api/dashboards/transfer", &body).await
}

/// File a dashboard under a project (or unfile it with `None`). A manifest-only edit — the
/// dashboard keeps running — that returns the fresh listing so the page regroups in one round-trip.
pub async fn set_dashboard_project(
    id: String,
    project: Option<String>,
) -> Result<DashboardsState, String> {
    post(
        "/api/dashboards/project",
        &SetDashboardProject { id, project },
    )
    .await
}

/// Every Hive service across all projects, with live running flags.
pub async fn hive() -> Result<HiveState, String> {
    get("/api/hive").await
}

pub async fn start_service(
    project: Option<String>,
    service: String,
) -> Result<StartResult, String> {
    post("/api/hive/start", &StartService { project, service }).await
}

pub async fn stop_service(project: Option<String>, service: String) -> Result<StopResult, String> {
    post("/api/hive/stop", &StartService { project, service }).await
}

/// Add a service to a project's `.adi/hive.yaml`; returns the fresh detail so the
/// project page updates in one round-trip.
pub async fn create_service(body: NewService) -> Result<ProjectDetail, String> {
    post("/api/hive/create", &body).await
}

// Project files: browse/read/edit the files under a project's own directory (jailed to it).

pub async fn list_files(id: &str, path: &str) -> Result<DirListing, String> {
    post(
        "/api/projects/files",
        &FilesRef {
            id: id.to_string(),
            path: path.to_string(),
        },
    )
    .await
}

pub async fn read_file(id: &str, path: &str) -> Result<FileContent, String> {
    post(
        "/api/projects/file/read",
        &FilesRef {
            id: id.to_string(),
            path: path.to_string(),
        },
    )
    .await
}

pub async fn write_file(id: &str, path: &str, content: &str) -> Result<FileContent, String> {
    post(
        "/api/projects/file/write",
        &WriteFile {
            id: id.to_string(),
            path: path.to_string(),
            content: content.to_string(),
        },
    )
    .await
}

// Workspaces & project hooks: working copies created by the project's .adi/hooks scripts.
// Every mutation returns (or carries) the fresh WorkspacesState for one-round-trip updates.

pub async fn workspaces(id: &str) -> Result<WorkspacesState, String> {
    post(
        "/api/projects/workspaces",
        &WorkspacesRef { id: id.to_string() },
    )
    .await
}

pub async fn create_workspace(body: NewWorkspace) -> Result<WorkspaceCreateResult, String> {
    post("/api/projects/workspaces/create", &body).await
}

pub async fn remove_workspace(id: String, name: String) -> Result<WorkspacesState, String> {
    post(
        "/api/projects/workspaces/remove",
        &WorkspaceRef { id, name },
    )
    .await
}

pub async fn run_project_hook(id: String, name: String) -> Result<ProjectHookRunResult, String> {
    post("/api/projects/hook/run", &ProjectHookRef { id, name }).await
}

pub async fn project_hook_log(id: String, name: String) -> Result<ProjectHookLog, String> {
    post("/api/projects/hook/log", &ProjectHookRef { id, name }).await
}

pub async fn create_project_hook(body: NewProjectHook) -> Result<WorkspacesState, String> {
    post("/api/projects/hook/create", &body).await
}

pub async fn open_workspace_terminal(id: String, name: String) -> Result<WorkspaceTerm, String> {
    post(
        "/api/projects/workspaces/terminal/open",
        &WorkspaceTermRef { id, name },
    )
    .await
}

pub async fn peek_workspace_terminal(id: String, name: String) -> Result<WorkspaceTerm, String> {
    post(
        "/api/projects/workspaces/terminal/peek",
        &WorkspaceTermRef { id, name },
    )
    .await
}

pub async fn send_workspace_terminal(
    id: String,
    name: String,
    text: String,
    key: String,
) -> Result<WorkspaceTerm, String> {
    post(
        "/api/projects/workspaces/terminal/send",
        &WorkspaceTermKeys {
            id,
            name,
            text,
            key,
        },
    )
    .await
}

pub async fn kill_workspace_terminal(id: String, name: String) -> Result<WorkspaceTerm, String> {
    post(
        "/api/projects/workspaces/terminal/kill",
        &WorkspaceTermRef { id, name },
    )
    .await
}

// The knowledge base: scoped collections of text notes, searched by meaning (docs/knowledge.md).

pub async fn knowledge() -> Result<KnowledgeState, String> {
    get("/api/knowledge").await
}

/// Search. An empty `bases` covers every base the caller may read, which is what the page's
/// search box sends — the query is embedded once however many bases it is put to.
pub async fn knowledge_search(
    query: String,
    bases: Vec<String>,
    words: bool,
) -> Result<KnowledgeResults, String> {
    post(
        "/api/knowledge/search",
        &KnowledgeSearch {
            query,
            bases,
            limit: None,
            text: words,
        },
    )
    .await
}

pub async fn knowledge_notes(base: String) -> Result<KnowledgeNotes, String> {
    post("/api/knowledge/notes", &base_ref(base)).await
}

pub async fn knowledge_note(base: String, id: String) -> Result<KnowledgeNoteDto, String> {
    post("/api/knowledge/note/get", &KnowledgeNoteRef { base, id }).await
}

pub async fn add_knowledge_note(body: NewKnowledgeNote) -> Result<KnowledgeSaved, String> {
    post("/api/knowledge/note/add", &body).await
}

pub async fn remove_knowledge_note(base: String, id: String) -> Result<KnowledgeNotes, String> {
    post("/api/knowledge/note/remove", &KnowledgeNoteRef { base, id }).await
}

pub async fn create_knowledge_base(body: NewKnowledgeBase) -> Result<KnowledgeState, String> {
    post("/api/knowledge/base/create", &body).await
}

pub async fn remove_knowledge_base(base: String) -> Result<KnowledgeState, String> {
    post("/api/knowledge/base/remove", &base_ref(base)).await
}

pub async fn reembed_knowledge(base: String) -> Result<KnowledgeReembed, String> {
    post("/api/knowledge/reembed", &base_ref(base)).await
}

/// The `{ base }` body four of the endpoints above share.
fn base_ref(base: String) -> KnowledgeBaseRef {
    KnowledgeBaseRef {
        base,
        tags: Vec::new(),
        limit: None,
    }
}

// Dictation. See `voice` for the capture that produces the clip.

/// Which speech engines the server can reach, and which of them have a key.
pub async fn voice() -> Result<VoiceState, String> {
    get("/api/voice").await
}

/// Send a recorded clip to be transcribed.
///
/// Raw bytes with the recorder's own `Content-Type`, not JSON: the clip is already bytes, and
/// base64 in a JSON field would add a third to a body that can run to megabytes for no gain.
pub async fn transcribe(engine: &str, mime: &str, audio: &[u8]) -> Result<Transcript, String> {
    let resp = Request::post(&format!("/api/voice/transcribe?engine={engine}"))
        .header("content-type", mime)
        // `Uint8Array::from` copies into the JS heap; the body must outlive this wasm frame and
        // a view onto wasm memory would dangle the moment the allocator moves it.
        .body(js_sys::Uint8Array::from(audio))
        .map_err(stringify)?
        .send()
        .await
        .map_err(stringify)?;
    finish(resp).await
}

/// Store one image for a message to carry, and get back the reference it is carried by.
///
/// Raw bytes for the same reason a dictated clip is raw bytes, plus one of its own: this is the
/// upload that happens while the message is still being typed, so it has to be as cheap as the
/// picture itself and not a third larger.
///
/// Routed exactly like every other agent call (`docs/fleet.md` §13): `routed_for` prefixes the
/// request with `/api/node/<node>` for a session on another machine, and that forwarder now
/// carries this one write with its own type and filename rather than wrapping it in JSON — so the
/// bytes land on the machine the run is actually on, which is what attaching a file is for.
pub async fn upload_attachment(
    node: Option<&str>,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<AgentAttachment, String> {
    let resp = Request::post(&routed_for(node, "/api/agents/attachment"))
        .header("content-type", mime)
        // The filename travels in a header because the body is the file. Percent-encoded: a
        // header is Latin-1 by the spec and a screenshot's name is routinely not.
        .header("x-adi-filename", &encode_header(name))
        .body(js_sys::Uint8Array::from(bytes))
        .map_err(stringify)?
        .send()
        .await
        .map_err(stringify)?;
    finish(resp).await
}

/// A filename reduced to what a header can carry: anything outside printable ASCII becomes `_`.
///
/// Not an encoding a server has to undo — the name is only ever shown back to the person who
/// attached it, so a mangled character costs nothing, while a raw one throws on the way out.
fn encode_header(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Which machine's agents these calls are about (`docs/fleet.md` §13)
// ---------------------------------------------------------------------------------------

/// Where a call to a specific source's agent API goes: that node's forwarding prefix, or the path
/// itself for this machine (`node: None`).
///
/// **Only the agent API moves**, and that is the whole scope of §13 rather than an accident of
/// where the prefix was easiest to add. The rest of the panel — projects, ports, the store browser,
/// the fleet page that named the node in the first place — is about *this* machine and its files.
///
/// A pure function rather than a thread-local flipped before a request and read inside it: the
/// sessions rail now asks several sources **concurrently** (`docs/fleet.md` §13, multi-select), and
/// a shared "current node" set by one in-flight call would be read by another's request the moment
/// an `.await` interleaved them. Every caller that needs a node now says so at the call, in
/// [`get_on`]/[`post_on`] — and in [`crate::live::Sub::get_on`]/[`Sub::post_on`] for the reads the
/// socket repeats — so two concurrent fetches for two different nodes can never cross wires.
pub(crate) fn routed_for(node: Option<&str>, path: &str) -> String {
    match node {
        Some(node) => format!("/api/node/{node}{path}"),
        None => path.to_string(),
    }
}

// ---------------------------------------------------------------------------------------
// The panel-wide source picker (`docs/fleet.md` §14)
// ---------------------------------------------------------------------------------------

thread_local! {
    /// The picker's target, mirrored here by `App`'s own effect over `State::panel_source` — see
    /// [`set_panel_source`]. `None` is this machine, the same meaning `None` carries everywhere
    /// else in this file.
    ///
    /// A thread-local rather than a parameter threaded through [`get`]/[`post`]'s every caller,
    /// unlike [`routed_for`]'s explicit `node`: the panel points at exactly **one** source at a
    /// time — there is no multi-select here, unlike §13's rail — so there is one answer to "where
    /// does an ordinary bare call go" and no concurrent fetch for a second source that a shared
    /// variable could be read by mid-flip the way K3 ruled out for `routed_for` itself.
    static PANEL_SOURCE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Point the panel's *bare* reads and writes at a paired node, or back at this machine (`None`).
///
/// Called from nowhere but `App`'s own effect over `State::panel_source` — every other page moves
/// this only by moving that signal, never by calling this directly, which is what keeps "where is
/// the panel pointed" one fact rather than two that could disagree. [`get_on`]/[`post_on`] and
/// [`routed_for`] never consult this at all: an explicit source still means exactly what it names,
/// whatever the picker is doing (`docs/fleet.md` §14's bare-vs-explicit rule, which is §13's K3
/// invariant kept rather than layered under a second routing rule).
pub(crate) fn set_panel_source(node: Option<String>) {
    PANEL_SOURCE.with(|slot| *slot.borrow_mut() = node);
}

/// The picker's current target, for a caller outside this file that needs to *name* it rather than
/// route through it — a destructive confirmation and the flash that follows a write both read this
/// (`crate::ui::confirm`/`apply_mutation`, `docs/fleet.md` §14/ADI-MONO-89), so an operator sees
/// which node a bare mutation is about to reach before and after it happens, not only in the
/// titlebar. Everything inside this file keeps calling the private [`get`]/[`post`], which reach the
/// same thread-local through [`routed_for`] rather than through this accessor.
pub(crate) fn panel_source() -> Option<String> {
    PANEL_SOURCE.with(|slot| slot.borrow().clone())
}

/// The ordinary bare `GET` a page makes when it isn't naming a source of its own — follows the
/// picker (`docs/fleet.md` §14). [`get_local`] is the same call without that, for the handful of
/// endpoints §14 lists as always local.
async fn get<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    get_at(&routed_for(panel_source().as_deref(), url)).await
}

/// [`get`], but never following the picker: `/api/health` and the status LED, `/api/fleet` and
/// `/api/fleet/*` (including the picker's own list), `/api/mesh*`, and the update/version pill
/// (`docs/fleet.md` §14) all call this instead.
async fn get_local<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    get_at(url).await
}

/// The ordinary bare `POST` a page makes when it isn't naming a source of its own — follows the
/// picker exactly as [`get`] does, mutation or not: §14 makes no distinction between a read and a
/// write here, because the picker points the whole panel and not half of it.
async fn post<B: Serialize, T: DeserializeOwned>(url: &str, body: &B) -> Result<T, String> {
    post_at(&routed_for(panel_source().as_deref(), url), body).await
}

/// [`post`], but never following the picker — see [`get_local`].
async fn post_local<B: Serialize, T: DeserializeOwned>(url: &str, body: &B) -> Result<T, String> {
    post_at(url, body).await
}

/// [`get`]/[`get_local`]/[`get_on`]'s shared plumbing: an address, already resolved, and nothing
/// else — the one place that actually sends a `GET`.
async fn get_at<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    let resp = Request::get(url).send().await.map_err(stringify)?;
    finish(resp).await
}

/// [`get_at`]'s `POST` counterpart.
async fn post_at<B: Serialize, T: DeserializeOwned>(url: &str, body: &B) -> Result<T, String> {
    let resp = Request::post(url)
        .json(body)
        .map_err(stringify)?
        .send()
        .await
        .map_err(stringify)?;
    finish(resp).await
}

/// [`get`], routed at a specific paired node (or this machine, for `None`) — never the picker's
/// target: an explicit source is what §13's K3 invariant and §14's bare-vs-explicit rule both
/// protect, so this reaches `get_at` directly rather than through [`get`].
async fn get_on<T: DeserializeOwned>(node: Option<&str>, path: &str) -> Result<T, String> {
    get_at(&routed_for(node, path)).await
}

/// [`post`], routed at a specific paired node (or this machine, for `None`) — see [`get_on`].
async fn post_on<B: Serialize, T: DeserializeOwned>(
    node: Option<&str>,
    path: &str,
    body: &B,
) -> Result<T, String> {
    post_at(&routed_for(node, path), body).await
}

/// Turn a response into `T`, or a message: the API's `{ error }` if present, else the
/// HTTP status line.
async fn finish<T: DeserializeOwned>(resp: Response) -> Result<T, String> {
    let status = resp.status();
    let text = resp.text().await.map_err(stringify)?;
    if !(200..300).contains(&status) {
        let msg = serde_json::from_str::<ApiError>(&text)
            .map_or_else(|_| format!("{status} {}", resp.status_text()), |e| e.error);
        return Err(msg);
    }
    serde_json::from_str(&text).map_err(stringify)
}

fn stringify<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// The ADI store browser: browse/read/edit everything under ~/.adi/mono (jailed to it).

pub async fn fs_list(path: &str) -> Result<FsListing, String> {
    post(
        "/api/fs/list",
        &FsRef {
            path: path.to_string(),
        },
    )
    .await
}

pub async fn fs_read(path: &str) -> Result<FsContent, String> {
    post(
        "/api/fs/read",
        &FsRef {
            path: path.to_string(),
        },
    )
    .await
}

pub async fn fs_write(path: &str, content: String) -> Result<FsContent, String> {
    post(
        "/api/fs/write",
        &FsWrite {
            path: path.to_string(),
            content,
        },
    )
    .await
}

/// Create an empty file or a directory in the store. The reply is the fresh listing of the
/// directory it landed in, so the tree redraws that folder without a second round-trip.
pub async fn fs_create(path: String, dir: bool) -> Result<FsListing, String> {
    post(
        "/api/fs/create",
        &FsCreate {
            path,
            kind: if dir { "dir" } else { "file" }.to_string(),
        },
    )
    .await
}
