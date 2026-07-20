//! Thin fetch layer over the `/api/*` endpoints, deserializing into the shared DTOs.

use adi_webapp_api::types::{
    AgentBuildResult, AgentCode, AgentKeys, AgentPeek, AgentRef, AgentRunResult, AgentRuns,
    AgentsState, ApiError, Dashboard, DashboardRef, DashboardsState, DirListing, FileContent,
    FilesRef,
    FsContent, FsCreate, FsListing, FsRef, FsWrite, Health, HiveState, LeaseRef, MeshForwardRef,
    MeshListenRef, MeshPeerRef, MeshPortRef, MeshState, MetaState, NewDashboard, NewProject,
    NewProjectHook,
    LinkTool, NewService, NewTask, NewTool, NewWorkspace, PortsState, ProjectDetail, ProjectHookLog,
    ProjectHookRef, ProjectHookRunResult, ProjectRef, ProjectsState, ReleaseResponse,
    ReserveResponse, RunAgent, RunRef, RunTool, SaveAgent, SaveAgentCode, SaveTrigger, StartResult,
    StartService, StopResult, TaskRef, TasksState, ToolRef, ToolRunResult, ToolScript, ToolsState,
    TriggerFireResult, TriggerLog, TriggerRef, TriggersState, UsedPorts, WorkspaceCreateResult,
    WorkspaceRef, WorkspaceTerm, WorkspaceTermKeys, WorkspaceTermRef, WorkspacesRef,
    WorkspacesState, WriteFile, WriteToolScript,
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

pub async fn run_agent(name: String, message: String) -> Result<AgentRunResult, String> {
    post("/api/agents/run", &RunAgent { name, message }).await
}

pub async fn stop_agent(name: String) -> Result<AgentsState, String> {
    post("/api/agents/stop", &AgentRef { name }).await
}

/// A headless agent's run history, newest first.
pub async fn agent_runs(name: String) -> Result<AgentRuns, String> {
    post("/api/agents/runs", &AgentRef { name }).await
}

/// A snapshot of one specific run's log.
pub async fn peek_run(name: String, run_id: String) -> Result<AgentPeek, String> {
    post("/api/agents/run/peek", &RunRef { name, run_id }).await
}

/// Stop one specific run, returning the fresh run history.
pub async fn stop_run(name: String, run_id: String) -> Result<AgentRuns, String> {
    post("/api/agents/run/stop", &RunRef { name, run_id }).await
}

pub async fn peek_agent(name: String) -> Result<AgentPeek, String> {
    post("/api/agents/peek", &AgentRef { name }).await
}

pub async fn send_agent_keys(name: String, text: String, key: String) -> Result<AgentPeek, String> {
    post("/api/agents/send-keys", &AgentKeys { name, text, key }).await
}

pub async fn agent_code(name: String) -> Result<AgentCode, String> {
    post("/api/agents/code", &AgentRef { name }).await
}

pub async fn save_agent_code(name: String, code: String) -> Result<AgentCode, String> {
    post("/api/agents/code/save", &SaveAgentCode { name, code }).await
}

pub async fn build_agent(name: String) -> Result<AgentBuildResult, String> {
    post("/api/agents/build", &AgentRef { name }).await
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
