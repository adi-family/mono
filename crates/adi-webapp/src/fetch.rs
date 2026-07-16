//! Thin fetch layer over the `/api/*` endpoints, deserializing into the shared DTOs.

use adi_webapp_api::types::{
    AgentKeys, AgentPeek, AgentRef, AgentRunResult, AgentsState, ApiError, DirListing, FileContent,
    FilesRef, Health, HiveState,
    LeaseRef, MeshForwardRef, MeshListenRef, MeshPeerRef, MeshPortRef, MeshState, NewProject,
    NewTask, PortsState, ProjectDetail, ProjectRef, ProjectsState, ReleaseResponse,
    ReserveResponse, SaveAgent, StartResult, StartService, StopResult, TasksState, UsedPorts,
    WriteFile,
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

pub async fn run_agent(name: String) -> Result<AgentRunResult, String> {
    post("/api/agents/run", &AgentRef { name }).await
}

pub async fn stop_agent(name: String) -> Result<AgentsState, String> {
    post("/api/agents/stop", &AgentRef { name }).await
}

pub async fn peek_agent(name: String) -> Result<AgentPeek, String> {
    post("/api/agents/peek", &AgentRef { name }).await
}

pub async fn send_agent_keys(name: String, text: String, key: String) -> Result<AgentPeek, String> {
    post("/api/agents/send-keys", &AgentKeys { name, text, key }).await
}

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
