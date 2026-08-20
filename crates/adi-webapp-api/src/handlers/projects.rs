use adi_projects::Error as ProjectStoreError;
use adi_projects::Projects;

use crate::types::{NewProject, Project, ProjectDetail, ProjectRef, ProjectsState, UsedPort};

use super::response::{FromBody, Response, error, mutate, ok_json};
use super::services::read_hive_services;

/// `GET /api/projects` — every registered project. Each mutation endpoint below returns a
/// fresh [`ProjectsState`], so the client refreshes from one round-trip.
#[must_use]
pub fn projects(store: &Projects) -> Response {
    match projects_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => Response::from(&e),
    }
}

/// The fresh project list as data rather than a response.
///
/// Public because the list is what *every* project mutation answers with, and one of them —
/// renaming a project, which spans stores this crate does not reach — is composed by the host
/// (`adi-app`). It still owes the client the same refreshed list, and there should be one place
/// that says what that list is.
///
/// # Errors
/// Whatever [`Projects::list`] returns.
pub fn projects_state(store: &Projects) -> Result<ProjectsState, ProjectStoreError> {
    Ok(ProjectsState {
        projects: store.list()?.into_iter().map(project_dto).collect(),
    })
}

/// `POST /api/projects/create` — register a project, then report the fresh list.
#[must_use]
pub fn create_project(store: &Projects, body: &[u8]) -> Response {
    mutate(
        body,
        |req: NewProject| store.create(req.name.trim(), req.description, req.parent),
        || projects(store),
    )
}

/// `POST /api/projects/archive` — archive a project (soft delete), then report the fresh list.
#[must_use]
pub fn archive_project(store: &Projects, body: &[u8]) -> Response {
    mutate(
        body,
        |req: ProjectRef| store.archive(req.id.trim()),
        || projects(store),
    )
}

/// `POST /api/projects/unarchive` — restore an archived project, then report the fresh list.
#[must_use]
pub fn unarchive_project(store: &Projects, body: &[u8]) -> Response {
    mutate(
        body,
        |req: ProjectRef| store.unarchive(req.id.trim()),
        || projects(store),
    )
}

/// `GET /api/projects/<id>` — one project's manifest plus the services parsed from its
/// `.adi/hive.yaml` (what's "inside" the project). `live` is the machine's listening TCP ports
/// with their sampled process usage (the host scans the platform and passes it), so each service
/// gets a live running flag and, when it's up, its CPU/memory.
#[must_use]
pub fn project_detail(store: &Projects, id: &str, live: &[UsedPort]) -> Response {
    let project = match store.get(id) {
        Ok(Some(project)) => project,
        Ok(None) => return error(404, &format!("no such project: {id}")),
        Err(e) => return Response::from(&e),
    };
    let (has_hive, services) = match store.hive_path(id) {
        Ok(path) => read_hive_services(&path, live),
        Err(e) => return Response::from(&e),
    };
    let subprojects = match store.children(id) {
        Ok(children) => children.into_iter().map(project_dto).collect(),
        Err(e) => return Response::from(&e),
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
pub fn remove_project(store: &Projects, body: &[u8]) -> Response {
    mutate(
        body,
        |req: ProjectRef| store.remove(req.id.trim()),
        || projects(store),
    )
}

// MARK: tasks — the task tree under ~/.adi/mono/tasks/tasks.json

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

// Map a store error to an HTTP status: bad id → 400, duplicate → 409, missing → 404, else 500.
impl From<&ProjectStoreError> for Response {
    fn from(e: &ProjectStoreError) -> Self {
        let status = match e {
            ProjectStoreError::InvalidId(_) => 400,
            ProjectStoreError::Exists(_) => 409,
            ProjectStoreError::NotFound(_) => 404,
            ProjectStoreError::Config(_) | ProjectStoreError::Io(_) => 500,
        };
        error(status, &e.to_string())
    }
}

impl FromBody for NewProject {
    const EXPECTED: &'static str =
        "expected JSON body { \"name\": \"…\", \"description\"?: \"…\", \"parent\"?: \"…\" }";

    fn is_complete(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

impl FromBody for ProjectRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

// MARK: triggers — background code blocks fired by webhooks & co. (~/.adi/mono/triggers)
