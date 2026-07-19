use adi_projects::Error as ProjectStoreError;
use adi_projects::Projects;

use crate::types::{NewProject, Project, ProjectDetail, ProjectRef, ProjectsState};

use super::response::{Response, error, ok_json};
use super::services::read_hive_services;

/// `GET /api/projects` — every registered project. Each mutation endpoint below returns a
/// fresh [`ProjectsState`], so the client refreshes from one round-trip.
#[must_use]
pub fn projects(store: &Projects) -> Response {
    match store.list() {
        Ok(list) => ok_json(&ProjectsState {
            projects: list.into_iter().map(project_dto).collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/projects/create` — register a project, then report the fresh list.
#[must_use]
pub fn create_project(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_new_project(body) else {
        return bad_new_project();
    };
    match store.create(req.name.trim(), req.description, req.parent) {
        Ok(_) => projects(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/projects/archive` — archive a project (soft delete), then report the fresh list.
#[must_use]
pub fn archive_project(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_project_ref(body) else {
        return bad_project_ref();
    };
    match store.archive(req.id.trim()) {
        Ok(_) => projects(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/projects/unarchive` — restore an archived project, then report the fresh list.
#[must_use]
pub fn unarchive_project(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_project_ref(body) else {
        return bad_project_ref();
    };
    match store.unarchive(req.id.trim()) {
        Ok(_) => projects(store),
        Err(e) => Response::from(&e),
    }
}

/// `GET /api/projects/<id>` — one project's manifest plus the services parsed from its
/// `.adi/hive.yaml` (what's "inside" the project). `listening` is the set of currently-listening
/// TCP ports (the host scans the platform and passes it), so each service gets a live running flag.
#[must_use]
pub fn project_detail(store: &Projects, id: &str, listening: &[u16]) -> Response {
    let project = match store.get(id) {
        Ok(Some(project)) => project,
        Ok(None) => return error(404, &format!("no such project: {id}")),
        Err(e) => return Response::from(&e),
    };
    let (has_hive, services) = match store.hive_path(id) {
        Ok(path) => read_hive_services(&path, listening),
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
    let Some(req) = parse_project_ref(body) else {
        return bad_project_ref();
    };
    match store.remove(req.id.trim()) {
        Ok(_) => projects(store),
        Err(e) => Response::from(&e),
    }
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

fn parse_new_project(body: &[u8]) -> Option<NewProject> {
    let req: NewProject = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn bad_new_project() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"description\"?: \"…\", \"parent\"?: \"…\" }",
    )
}

fn parse_project_ref(body: &[u8]) -> Option<ProjectRef> {
    let req: ProjectRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty()).then_some(req)
}

fn bad_project_ref() -> Response {
    error(400, "expected JSON body { \"id\": \"…\" }")
}

// MARK: triggers — background code blocks fired by webhooks & co. (~/.adi/mono/triggers)
