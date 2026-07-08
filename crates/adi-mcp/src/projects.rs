//! The `projects` feature: CRUD over the adi projects registry, wrapping
//! [`adi_core::Adi::projects`]. These tools own no logic of their own — they are a thin MCP
//! adapter over the same registry the CLI and the app drive, so behaviour (id validation,
//! archive semantics) stays identical across every frontend.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::{AdiMcp, json_result};

/// Arguments for `projects_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ListProjectsArgs {
    /// Include archived projects too (default: only active projects are returned).
    #[serde(default)]
    include_archived: bool,
}

/// Arguments for `projects_get`, `projects_archive`, and `projects_unarchive`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ProjectIdArgs {
    /// The project id (its directory name in the registry).
    id: String,
}

/// Arguments for `projects_create`.
#[derive(Debug, Deserialize, JsonSchema)]
struct CreateProjectArgs {
    /// The project id — a single safe path segment (letters, digits, `.`, `-`, `_`).
    id: String,
    /// Optional display name (defaults to the id).
    #[serde(default)]
    name: Option<String>,
    /// Optional one-line description.
    #[serde(default)]
    description: Option<String>,
}

#[tool_router(router = projects_router, vis = "pub")]
impl AdiMcp {
    #[tool(description = "List registered adi projects (active only unless include_archived is set)")]
    async fn projects_list(
        &self,
        Parameters(args): Parameters<ListProjectsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut projects = adi_core::Adi::new()
            .projects()
            .list()
            .map_err(project_err)?;
        if !args.include_archived {
            projects.retain(|p| !p.is_archived());
        }
        json_result(&projects)
    }

    #[tool(description = "Get a single adi project by id")]
    async fn projects_get(
        &self,
        Parameters(args): Parameters<ProjectIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        match adi_core::Adi::new().projects().get(&args.id).map_err(project_err)? {
            Some(project) => json_result(&project),
            None => Err(McpError::invalid_params(
                format!("no project with id {:?}", args.id),
                None,
            )),
        }
    }

    #[tool(description = "Register a new adi project")]
    async fn projects_create(
        &self,
        Parameters(args): Parameters<CreateProjectArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = adi_core::Adi::new()
            .projects()
            .create(&args.id, args.name, args.description)
            .map_err(project_err)?;
        json_result(&project)
    }

    #[tool(description = "Archive (soft-delete) an adi project")]
    async fn projects_archive(
        &self,
        Parameters(args): Parameters<ProjectIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = adi_core::Adi::new()
            .projects()
            .archive(&args.id)
            .map_err(project_err)?;
        json_result(&project)
    }

    #[tool(description = "Restore an archived adi project")]
    async fn projects_unarchive(
        &self,
        Parameters(args): Parameters<ProjectIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = adi_core::Adi::new()
            .projects()
            .unarchive(&args.id)
            .map_err(project_err)?;
        json_result(&project)
    }
}

/// Map an [`adi_core::ProjectsError`] to a fitting MCP error: bad id, already-exists, and
/// not-found are client (`invalid_params`) errors; store I/O and TOML failures are internal.
// Consumed by value because it is used as a `.map_err(project_err)` adapter, which hands the
// error over by value.
#[allow(clippy::needless_pass_by_value)]
fn project_err(e: adi_core::ProjectsError) -> McpError {
    use adi_core::ProjectsError as E;
    match e {
        E::InvalidId(_) | E::Exists(_) | E::NotFound(_) => {
            McpError::invalid_params(e.to_string(), None)
        }
        E::Config(_) | E::Io(_) => McpError::internal_error(format!("projects error: {e}"), None),
    }
}
