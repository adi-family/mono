//! `GET /api/tools` and its mutations — user CLIs under `~/.adi/mono/tools`, run by agents.
//!
//! A tool is a small sh/ts script that is either **owned** (its code lives in the store) or
//! **linked** (a manifest pointing at an existing file). Each active tool is exposed as a
//! `tools/.bin/<name>` shim (see [`adi_tools`]); creating, editing, archiving, or removing a tool
//! regenerates those shims, so an agent with that `.bin` on its PATH always sees the live set.

use adi_projects::Projects;
use adi_tools::Error as ToolStoreError;
use adi_tools::{Tool, Tools};

use crate::types::{
    LinkTool, NewTool, RunTool, ToolDto, ToolRef, ToolRunResult, ToolScript, ToolsState,
    WriteToolScript,
};

use super::response::{Response, error, ok_json};

/// The largest script we'll accept on a write — the same bound the project file editor uses.
/// Comfortably under the server's 1 MiB request-body cap; a larger script is refused, not truncated.
const MAX_SCRIPT_BYTES: usize = 512 * 1024;

/// `GET /api/tools` — every registered tool plus the `.bin` directory. Each mutation endpoint
/// below returns a fresh [`ToolsState`], so the client refreshes from one round-trip.
#[must_use]
pub fn tools(store: &Tools) -> Response {
    match store.list() {
        Ok(list) => ok_json(&ToolsState {
            tools: list.into_iter().map(tool_dto).collect(),
            bin_dir: store.bin_dir().display().to_string(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/tools/create` — register an owned tool (writes its script), then report the fresh list.
#[must_use]
pub fn create_tool(store: &Tools, body: &[u8]) -> Response {
    let req: NewTool = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(_) => {
            return error(
                400,
                "expected JSON body { \"name\": \"…\", \"runtime\": \"sh|ts\" }",
            );
        }
    };
    if req.name.trim().is_empty() {
        return error(400, "a tool name is required");
    }
    match store.create_file(
        req.name.trim(),
        req.description,
        &req.runtime,
        req.project,
        req.content,
    ) {
        Ok(_) => tools(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/tools/link` — register a tool that links an existing file, then report the fresh list.
#[must_use]
pub fn link_tool(store: &Tools, body: &[u8]) -> Response {
    let req: LinkTool = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(_) => return error(400, "expected JSON body { \"path\": \"…\" }"),
    };
    if req.path.trim().is_empty() {
        return error(400, "a path is required");
    }
    match store.link(
        req.path.trim(),
        req.name,
        req.runtime,
        req.description,
        req.project,
    ) {
        Ok(_) => tools(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/tools/archive` — archive a tool (soft delete; drops its `.bin` shim).
#[must_use]
pub fn archive_tool(store: &Tools, body: &[u8]) -> Response {
    let Some(id) = parse_tool_ref(body) else {
        return bad_tool_ref();
    };
    match store.archive(&id) {
        Ok(_) => tools(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/tools/unarchive` — restore an archived tool (re-creates its `.bin` shim).
#[must_use]
pub fn unarchive_tool(store: &Tools, body: &[u8]) -> Response {
    let Some(id) = parse_tool_ref(body) else {
        return bad_tool_ref();
    };
    match store.unarchive(&id) {
        Ok(_) => tools(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/tools/remove` — permanently delete a tool (a linked target is never touched).
#[must_use]
pub fn remove_tool(store: &Tools, body: &[u8]) -> Response {
    let Some(id) = parse_tool_ref(body) else {
        return bad_tool_ref();
    };
    match store.remove(&id) {
        Ok(_) => tools(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/tools/script/read` — a tool's script text (owned file, or linked target).
#[must_use]
pub fn read_tool_script(store: &Tools, body: &[u8]) -> Response {
    let Some(id) = parse_tool_ref(body) else {
        return bad_tool_ref();
    };
    script_response(store, &id)
}

/// `POST /api/tools/script/write` — overwrite a tool's script, then re-read it so the response
/// carries the authoritative content after the write.
#[must_use]
pub fn write_tool_script(store: &Tools, body: &[u8]) -> Response {
    let req: WriteToolScript = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(_) => {
            return error(400, "expected JSON body { \"id\": \"…\", \"content\": \"…\" }");
        }
    };
    if req.id.trim().is_empty() {
        return bad_tool_ref();
    }
    if req.content.len() > MAX_SCRIPT_BYTES {
        return error(
            413,
            &format!("script too large to save (max {MAX_SCRIPT_BYTES} bytes)"),
        );
    }
    if let Err(e) = store.write_script(req.id.trim(), &req.content) {
        return Response::from(&e);
    }
    script_response(store, req.id.trim())
}

/// `POST /api/tools/run` — run a tool once and return its captured output plus the fresh list.
/// A project-scoped tool runs in its project directory (resolved via `projects`), a global tool
/// in the store root.
#[must_use]
pub fn run_tool(store: &Tools, projects: &Projects, body: &[u8]) -> Response {
    let req: RunTool = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(_) => return error(400, "expected JSON body { \"id\": \"…\", \"args\"?: [\"…\"] }"),
    };
    if req.id.trim().is_empty() {
        return bad_tool_ref();
    }
    let id = req.id.trim();

    // Resolve the working directory from the tool's project, if any.
    let cwd = match store.get(id) {
        Ok(Some(tool)) => tool
            .manifest
            .project
            .as_deref()
            .and_then(|p| projects.project_dir(p).ok()),
        Ok(None) => return error(404, &format!("no such tool: {id}")),
        Err(e) => return Response::from(&e),
    };

    match store.run(id, &req.args, cwd.as_deref()) {
        Ok(out) => {
            let Response { status, body } = tools(store);
            // `tools` only ever returns a serialized ToolsState here; fold it into the run result.
            if status != 200 {
                return Response { status, body };
            }
            match serde_json::from_str::<ToolsState>(&body) {
                Ok(state) => ok_json(&ToolRunResult {
                    id: id.to_string(),
                    exit_code: out.code,
                    ok: out.ok(),
                    output: out.output,
                    state,
                }),
                Err(e) => error(500, &format!("serializing tools state: {e}")),
            }
        }
        Err(e) => Response::from(&e),
    }
}

/// Read a tool's script and shape a [`ToolScript`] response.
fn script_response(store: &Tools, id: &str) -> Response {
    let tool = match store.get(id) {
        Ok(Some(tool)) => tool,
        Ok(None) => return error(404, &format!("no such tool: {id}")),
        Err(e) => return Response::from(&e),
    };
    let path = match store.script_path(id) {
        Ok(path) => path.display().to_string(),
        Err(e) => return Response::from(&e),
    };
    match store.read_script(id) {
        Ok(content) => ok_json(&ToolScript {
            id: id.to_string(),
            path,
            content,
            runtime: tool.runtime().to_string(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// Flatten a stored [`Tool`] into its wire [`ToolDto`].
fn tool_dto(tool: Tool) -> ToolDto {
    let bin_name = tool.bin_name();
    let runtime = tool.runtime().to_string();
    let linked = tool.is_linked();
    let system = tool.is_system();
    ToolDto {
        id: tool.id,
        name: tool.manifest.name,
        description: tool.manifest.description,
        runtime,
        linked,
        path: tool.manifest.linked_path,
        bin_name,
        project: tool.manifest.project,
        system,
        created_at: tool.manifest.created_at,
        archived_at: tool.manifest.archived_at,
    }
}

// Map a tool-store error to an HTTP status: bad id/runtime → 400, missing tool/linked file → 404,
// a protected system tool → 409, launch/store failure → 500.
impl From<&ToolStoreError> for Response {
    fn from(e: &ToolStoreError) -> Self {
        let status = match e {
            ToolStoreError::InvalidId(_) | ToolStoreError::InvalidRuntime(_) => 400,
            ToolStoreError::NotFound(_) | ToolStoreError::LinkedMissing(_) => 404,
            ToolStoreError::SystemProtected(_) => 409,
            ToolStoreError::Config(_) | ToolStoreError::Io(_) | ToolStoreError::Launch(_) => 500,
        };
        error(status, &e.to_string())
    }
}

fn parse_tool_ref(body: &[u8]) -> Option<String> {
    let req: ToolRef = serde_json::from_slice(body).ok()?;
    let id = req.id.trim().to_string();
    (!id.is_empty()).then_some(id)
}

fn bad_tool_ref() -> Response {
    error(400, "expected JSON body { \"id\": \"…\" }")
}
