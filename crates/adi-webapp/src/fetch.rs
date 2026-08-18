//! Thin fetch layer over the `/api/*` endpoints, deserializing into the shared DTOs.

use adi_webapp_api::types::{
    AgentAttachment, AgentGoals, AgentKeys, AgentPeek, AgentRef, AgentReviewStarted, AgentRunResult, AgentRuns,
    AgentSimBlock, AgentSimState, AgentSimTurn, AgentTokens,
    AgentsState, AllAgentRuns, AnswerRun, ApiError, CloseGoal, Dashboard, DashboardRef,
    DashboardTransferred,
    DashboardsState, DbExecResult,
    DbQuery, DbQueryResult, DbSchema, DbScope, DbState, DbTablesState, DirListing, FileContent,
    FilesRef, FleetDashboards, FleetGrantRef, FleetRef, FleetRename, FleetState, FsContent,
    FsCreate, FsListing, FsRef, FsWrite, Health, HideRun, HiveState,
    KnowledgeBaseRef, KnowledgeNoteDto, KnowledgeNoteRef, KnowledgeNotes, KnowledgeReembed,
    KnowledgeResults, KnowledgeSaved, KnowledgeSearch, KnowledgeState, LeaseRef,
    LinkTool, MeshForwardRef, MeshListenRef, MeshPeerRef, MeshPortRef, MeshState, MetaState,
    NewDashboard, NewKnowledgeBase, NewKnowledgeNote, NewProject, NewProjectHook, NewService,
    NewTask, NewTool, NewWorkspace,
    NodeServiceRef,
    PortsState, ProjectDetail, ProjectHookLog, ProjectHookRef, ProjectHookRunResult, ProjectRef,
    GoalsOf, ProjectsState, ReleaseResponse, ReplyToRun, ReserveResponse, RevealedSecret, ReviewRun,
    SimulateAgent, SimulateTurn,
    RunAgent, RunRef,
    RunTool, SaveAgent, SaveTrigger, SecretRef, SecretsState, SetDashboardProject,
    SetGoal, SetOAuthSecret, SetRunLimit, SetSecret, StarRun, StartResult, StartService, StopResult,
    TaskRef,
    TasksState, ToolRef, ToolRunResult, ToolScript, ToolsState, TransferDashboard,
    TriggerFireResult, TriggerLog,
    Transcript,
    TriggerRef, TriggersState, UnlockNode, UnqueueFromRun, UsedPorts, WorkspaceCreateResult,
    WorkspaceRef,
    VoiceState,
    WorkspaceTerm, WorkspaceTermKeys, WorkspaceTermRef, WorkspacesRef, WorkspacesState, WriteFile,
    WriteToolScript,
};
use gloo_net::http::{Request, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub async fn health() -> Result<Health, String> {
    get("/api/health").await
}

pub async fn ports() -> Result<PortsState, String> {
    get("/api/ports").await
}

pub async fn used() -> Result<UsedPorts, String> {
    get("/api/ports/used").await
}

/// The Meta page's state: the well-known `adi-agent` (if set up), the default system prompt, and
/// the agent form schema. Creating/running it reuses the `save_agent` / `run_agent` endpoints.
pub async fn meta() -> Result<MetaState, String> {
    get("/api/meta").await
}

pub async fn reserve(body: &LeaseRef) -> Result<ReserveResponse, String> {
    post("/api/ports/reserve", body).await
}

pub async fn release(body: &LeaseRef) -> Result<ReleaseResponse, String> {
    post("/api/ports/release", body).await
}

// Mesh: every endpoint returns the fresh MeshState so the page updates in one round-trip.

pub async fn mesh() -> Result<MeshState, String> {
    get("/api/mesh").await
}

pub async fn mesh_start() -> Result<MeshState, String> {
    post("/api/mesh/start", &()).await
}

pub async fn mesh_stop() -> Result<MeshState, String> {
    post("/api/mesh/stop", &()).await
}

pub async fn mesh_allow(port: u16) -> Result<MeshState, String> {
    post("/api/mesh/allow", &MeshPortRef { port }).await
}

pub async fn mesh_deny(port: u16) -> Result<MeshState, String> {
    post("/api/mesh/deny", &MeshPortRef { port }).await
}

pub async fn mesh_allow_peer(peer: String) -> Result<MeshState, String> {
    post("/api/mesh/peers/allow", &MeshPeerRef { peer }).await
}

pub async fn mesh_deny_peer(peer: String) -> Result<MeshState, String> {
    post("/api/mesh/peers/deny", &MeshPeerRef { peer }).await
}

pub async fn mesh_add_forward(body: MeshForwardRef) -> Result<MeshState, String> {
    post("/api/mesh/forwards/add", &body).await
}

pub async fn mesh_remove_forward(listen: u16) -> Result<MeshState, String> {
    post("/api/mesh/forwards/remove", &MeshListenRef { listen }).await
}

// Fleet: the paired remote nodes. As with mesh, every endpoint answers with the fresh
// FleetState, so an edit and the view of it are one round-trip.

pub async fn fleet() -> Result<FleetState, String> {
    get("/api/fleet").await
}

pub async fn fleet_rename(petname: String, to: String) -> Result<FleetState, String> {
    post("/api/fleet/rename", &FleetRename { petname, to }).await
}

pub async fn fleet_unpair(petname: String) -> Result<FleetState, String> {
    post("/api/fleet/unpair", &FleetRef { petname }).await
}

pub async fn fleet_grant(petname: String, grant: String) -> Result<FleetState, String> {
    post("/api/fleet/grants/add", &FleetGrantRef { petname, grant }).await
}

pub async fn fleet_revoke(petname: String, grant: String) -> Result<FleetState, String> {
    post(
        "/api/fleet/grants/remove",
        &FleetGrantRef { petname, grant },
    )
    .await
}

pub async fn fleet_accept_nickname(petname: String) -> Result<FleetState, String> {
    post("/api/fleet/nickname/accept", &FleetRef { petname }).await
}

pub async fn fleet_dismiss_nickname(petname: String) -> Result<FleetState, String> {
    post("/api/fleet/nickname/dismiss", &FleetRef { petname }).await
}

// The fleet's dashboards: what each paired node runs, asked of that node's own control panel over
// the mesh. Every one of these leaves the machine, so they are slower than the calls above — the
// caller should show the rail as loading rather than assume a local round-trip. Each answers with
// the whole fresh listing, the same one-round-trip contract the rest of `/api/fleet` keeps.

pub async fn fleet_dashboards() -> Result<FleetDashboards, String> {
    get("/api/fleet/dashboards").await
}

/// Give this machine a node's password, so that node's dashboards can be listed. Checked against
/// the node before it is stored, so a rejected password comes back as an error here rather than as
/// a broken row later.
pub async fn unlock_node(node: String, password: String) -> Result<FleetDashboards, String> {
    post(
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
    post("/api/fleet/dashboards/forget", &FleetRef { petname }).await
}

/// Ask a node to let this machine reach one of its services (`http:<service>`), so a listed
/// dashboard becomes a link that opens rather than one that refuses.
pub async fn allow_node_service(node: String, service: String) -> Result<FleetDashboards, String> {
    post(
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

// Agents: every endpoint returns the fresh AgentsState so the page updates in one round-trip.

pub async fn agents() -> Result<AgentsState, String> {
    get("/api/agents").await
}

pub async fn save_agent(body: SaveAgent) -> Result<AgentsState, String> {
    post("/api/agents/save", &body).await
}

pub async fn delete_agent(name: String) -> Result<AgentsState, String> {
    post("/api/agents/delete", &AgentRef { name }).await
}

/// Launch a run. `working_dir` is the composer's optional "run here" — blank means "run this agent
/// as defined", so it starts where its manifest and its project say. `force` launches past a full
/// concurrency limit — what the "Run anyway" affordance sends.
pub async fn run_agent(
    name: String,
    message: String,
    working_dir: Option<String>,
    force: bool,
    attachments: Vec<String>,
) -> Result<AgentRunResult, String> {
    post(
        "/api/agents/run",
        &RunAgent {
            name,
            message,
            working_dir,
            force,
            attachments,
            // The composer launches what a person typed; a pre-run is something a launcher that
            // already knows the agent's first move sends (the CLI's `--pre-run`, a filer's API
            // call). Nothing in this form offers one, so it sends none.
            pre_run: Vec::new(),
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

pub async fn stop_agent(name: String) -> Result<AgentsState, String> {
    post("/api/agents/stop", &AgentRef { name }).await
}

/// A headless agent's run history, newest first.
pub async fn agent_runs(name: String) -> Result<AgentRuns, String> {
    post("/api/agents/runs", &AgentRef { name }).await
}

/// Every agent's run history in one call — the data behind the cross-agent "All chats" index.
///
/// `limit` asks for only the newest N sessions across every agent, the page the chat rail opens
/// on; `None` is the whole history, which is what Analytics and the Agents index read. The answer
/// carries `total` either way, so a paged caller knows whether there is more behind it.
pub async fn all_agent_runs(limit: Option<usize>) -> Result<AllAgentRuns, String> {
    get(&all_runs_path(limit)).await
}

/// The `/api/agents/runs/all` request for a given page size — one function, because the live
/// channel watches this path by string and a subscription that spelled it differently would be a
/// second topic answering the same question.
pub fn all_runs_path(limit: Option<usize>) -> String {
    match limit {
        Some(n) => format!("/api/agents/runs/all?limit={n}"),
        None => "/api/agents/runs/all".to_string(),
    }
}

/// A snapshot of one specific run's log (plus the conversation transcript, for harness runs).
pub async fn peek_run(name: String, run_id: String) -> Result<AgentPeek, String> {
    post("/api/agents/run/peek", &RunRef { name, run_id }).await
}

/// The itemization of one conversation's context: how its tokens split by source, and which runs of
/// text were sent more than once. Asked for once, when the reader opens the panel — it re-tokenizes
/// the transcript and has no business on the one-second poll.
pub async fn run_tokens(name: String, run_id: String) -> Result<AgentTokens, String> {
    post("/api/agents/run/tokens", &RunRef { name, run_id }).await
}

/// Open a run of an agent with a person in the model's seat. Always a fresh run.
pub async fn simulate_agent(name: String, message: String) -> Result<AgentSimState, String> {
    post("/api/agents/simulate", &SimulateAgent { name, message }).await
}

/// The simulated run as the model sees it: the composed prompt, its split, its tools, its turns.
pub async fn simulate_prompt(name: String, run_id: String) -> Result<AgentSimState, String> {
    post("/api/agents/simulate/prompt", &RunRef { name, run_id }).await
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
        },
    )
    .await
}

/// Hand one conversation to the root agent and ask how the workflow should have gone. Writes the
/// dossier server-side and launches the reviewer on it; what comes back is where to watch, not the
/// review itself — the review is a conversation, and it is only starting.
pub async fn review_run(name: String, run_id: String) -> Result<AgentReviewStarted, String> {
    post(
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
/// behind the answer still in flight. Returns a fresh snapshot with the updated transcript
/// (including the streaming answer and anything queued).
pub async fn reply_to_run(
    name: String,
    run_id: String,
    message: String,
    attachments: Vec<String>,
) -> Result<AgentPeek, String> {
    post(
        "/api/agents/run/reply",
        &ReplyToRun {
            name,
            run_id,
            message,
            attachments,
        },
    )
    .await
}

/// Settle the question a conversation is waiting on, with one reply per question in the order they
/// were asked. `ask` names the ask so a card left open in another tab cannot answer the question
/// that has since replaced it — a stale one comes back 404, which is the useful answer.
pub async fn answer_run(
    name: String,
    run_id: String,
    ask: String,
    replies: Vec<String>,
) -> Result<AgentPeek, String> {
    post(
        "/api/agents/run/answer",
        &AnswerRun {
            name,
            run_id,
            ask: Some(ask),
            replies,
        },
    )
    .await
}

/// One conversation's goals, open and closed alike — what it is for, and what it already settled.
pub async fn agent_goals(name: String, run_id: String) -> Result<AgentGoals, String> {
    post("/api/agents/goals", &GoalsOf { name, run_id }).await
}

/// Write a goal onto a conversation, or reword one that is open (`goal` names which).
///
/// A goal set here is always recorded as set by a person: a run setting its own goes through the
/// CLI from inside its turn, and the two are worth telling apart afterward.
pub async fn set_agent_goal(
    name: String,
    run_id: String,
    text: String,
    goal: Option<String>,
) -> Result<AgentGoals, String> {
    post(
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
    goal: String,
    as_: String,
    note: String,
) -> Result<AgentGoals, String> {
    post("/api/agents/goal/close", &CloseGoal { goal, as_, note }).await
}

/// Drop the message at `index` from a conversation's queue, returning the fresh snapshot.
pub async fn unqueue_from_run(
    name: String,
    run_id: String,
    index: usize,
) -> Result<AgentPeek, String> {
    post(
        "/api/agents/run/unqueue",
        &UnqueueFromRun {
            name,
            run_id,
            index,
        },
    )
    .await
}

/// Stop one specific run, returning the fresh run history.
pub async fn stop_run(name: String, run_id: String) -> Result<AgentRuns, String> {
    post("/api/agents/run/stop", &RunRef { name, run_id }).await
}

/// Delete one run outright — for a harness agent, the whole conversation — returning the fresh run
/// history without it.
pub async fn delete_run(name: String, run_id: String) -> Result<AgentRuns, String> {
    post("/api/agents/run/delete", &RunRef { name, run_id }).await
}

/// Hide one session from the chat rail, or bring it back (`hidden: false`). Nothing is deleted and
/// nothing is stopped — the fresh run history still carries the run, now flagged `hidden`.
pub async fn hide_run(name: String, run_id: String, hidden: bool) -> Result<AgentRuns, String> {
    post(
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
/// one the per-agent cap will not sweep, so this is how a chat is kept past the fifty newest.
pub async fn star_run(name: String, run_id: String, starred: bool) -> Result<AgentRuns, String> {
    post(
        "/api/agents/run/star",
        &StarRun {
            name,
            run_id,
            starred,
        },
    )
    .await
}

pub async fn peek_agent(name: String) -> Result<AgentPeek, String> {
    post("/api/agents/peek", &AgentRef { name }).await
}

pub async fn send_agent_keys(name: String, text: String, key: String) -> Result<AgentPeek, String> {
    post("/api/agents/send-keys", &AgentKeys { name, text, key }).await
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
pub async fn upload_attachment(
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<AgentAttachment, String> {
    let resp = Request::post("/api/agents/attachment")
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

async fn get<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    let resp = Request::get(url).send().await.map_err(stringify)?;
    finish(resp).await
}

async fn post<B: Serialize, T: DeserializeOwned>(url: &str, body: &B) -> Result<T, String> {
    let resp = Request::post(url)
        .json(body)
        .map_err(stringify)?
        .send()
        .await
        .map_err(stringify)?;
    finish(resp).await
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
