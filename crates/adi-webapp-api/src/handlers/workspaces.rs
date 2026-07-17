use std::path::PathBuf;

use adi_hooks::Error as HookStoreError;
use adi_hooks::Hooks as ProjectHooks;
use adi_hooks::Workspaces;
use adi_hooks::hook_template;
use adi_hooks::is_lifecycle;
use adi_hooks::terminal;
use adi_projects::Projects;

use crate::types::{NewProjectHook, NewWorkspace, ProjectHookDto, ProjectHookLog, ProjectHookRef, ProjectHookRunResult, WorkspaceCreateResult, WorkspaceDto, WorkspaceRef, WorkspaceTerm, WorkspaceTermKeys, WorkspaceTermRef, WorkspacesRef, WorkspacesState};

use super::response::{error, ok_json, Response};

/// `POST /api/projects/workspaces` — a project's workspaces and hooks in one snapshot. Every
/// mutation in this family returns a fresh [`WorkspacesState`] for one-round-trip refreshes.
#[must_use]
pub fn workspaces_state(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_workspaces_ref(body) else {
        return error(400, "expected JSON body { \"id\": \"…\" }");
    };
    match build_workspaces_state(store, req.id.trim()) {
        Ok(state) => ok_json(&state),
        Err(resp) => resp,
    }
}

/// `POST /api/projects/workspaces/create` — create a workspace. The project's first
/// hook-backed workspace runs the `init` hook (e.g. `git clone`), each additional one the
/// `workspace` hook (e.g. `git worktree add`) — detached, so the response's state shows it
/// `creating`; with `local`, an existing directory is linked as-is and no hook runs.
#[must_use]
pub fn create_workspace(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_new_workspace(body) else {
        return error(
            400,
            "expected JSON body { \"id\": \"…\", \"name\": \"…\", \"path\"?: \"…\", \"local\"?: bool }",
        );
    };
    let id = req.id.trim();
    let (dir, env) = match project_scope(store, id) {
        Ok(scope) => scope,
        Err(resp) => return resp,
    };
    let explicit = req
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let name = req.name.trim();
    let (entry, run) =
        match Workspaces::new(&dir).create(name, explicit.as_deref(), req.local, &env) {
            Ok(created) => created,
            Err(e) => return Response::from(&e),
        };
    let message = match &run {
        Some(run) => format!(
            "Creating workspace “{name}” via the {} hook (pid {}).",
            entry.hook.as_deref().unwrap_or("?"),
            run.pid
        ),
        None => format!("Linked local workspace “{name}”."),
    };
    match build_workspaces_state(store, id) {
        Ok(state) => ok_json(&WorkspaceCreateResult { message, state }),
        Err(resp) => resp,
    }
}

/// `POST /api/projects/workspaces/remove` — unregister a workspace. Never touches its files;
/// a clone/worktree on disk stays where it is. An unknown name is a 404.
#[must_use]
pub fn remove_workspace(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_workspace_ref(body) else {
        return bad_project_hook_ref();
    };
    let id = req.id.trim();
    let (dir, _) = match project_scope(store, id) {
        Ok(scope) => scope,
        Err(resp) => return resp,
    };
    let name = req.name.trim();
    match Workspaces::new(&dir).remove(name) {
        Ok(true) => {}
        Ok(false) => return error(404, &format!("no such workspace: {name}")),
        Err(e) => return Response::from(&e),
    }
    match build_workspaces_state(store, id) {
        Ok(state) => ok_json(&state),
        Err(resp) => resp,
    }
}

/// `POST /api/projects/hook/run` — run a hook by hand, detached, with the project env and
/// cwd at the project directory. Replies with the spawned pid plus fresh state.
#[must_use]
pub fn run_project_hook(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_project_hook_ref(body) else {
        return bad_project_hook_ref();
    };
    let id = req.id.trim();
    let (dir, mut env) = match project_scope(store, id) {
        Ok(scope) => scope,
        Err(resp) => return resp,
    };
    env.push(("ADI_PROJECT_DIR".to_string(), dir.display().to_string()));
    let name = req.name.trim();
    // The lifecycle hooks get their ADI_WORKSPACE_* env only from a workspace create; a
    // bare run would see an empty $ADI_WORKSPACE_DIR and fail confusingly, so refuse it
    // with the actionable path instead.
    if is_lifecycle(name) {
        return error(
            409,
            &format!("the {name} hook runs when a workspace is created — use “Add workspace”"),
        );
    }
    let hooks = ProjectHooks::new(&dir);
    // A manual run of an unknown hook is a plain 404 (NoHook's 409 is for the lifecycle
    // hooks a workspace create depends on).
    if !hooks.exists(name) {
        return error(404, &format!("no such hook: {name}"));
    }
    let run = match hooks.run(name, &env, &dir) {
        Ok(run) => run,
        Err(e) => return Response::from(&e),
    };
    match build_workspaces_state(store, id) {
        Ok(state) => ok_json(&ProjectHookRunResult {
            message: format!("Running hook “{name}” (pid {}).", run.pid),
            state,
        }),
        Err(resp) => resp,
    }
}

/// `POST /api/projects/hook/log` — the tail of a hook's most recent run log. A hook that
/// never ran answers `ran: false` (200, not an error); only an unknown hook file is a 404.
#[must_use]
pub fn project_hook_log(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_project_hook_ref(body) else {
        return bad_project_hook_ref();
    };
    let id = req.id.trim();
    let (dir, _) = match project_scope(store, id) {
        Ok(scope) => scope,
        Err(resp) => return resp,
    };
    let hooks = ProjectHooks::new(&dir);
    let name = req.name.trim();
    if !hooks.exists(name) {
        return error(404, &format!("no such hook: {name}"));
    }
    let output = hooks.read_log(name);
    let status = hooks.status(name);
    ok_json(&ProjectHookLog {
        id: id.to_string(),
        name: name.to_string(),
        ran: output.is_some(),
        output: output.unwrap_or_default(),
        status: status.as_str().to_string(),
        exit_code: status.exit_code(),
        ran_at: hooks.last_run(name),
    })
}

/// `POST /api/projects/hook/create` — materialize a hook file from a template (`init` |
/// `workspace` | `blank`, the default). Refuses to overwrite (409) — edits go through the
/// project file browser, where the file lives at `.adi/hooks/<name>`.
#[must_use]
pub fn create_project_hook(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_new_project_hook(body) else {
        return error(
            400,
            "expected JSON body { \"id\": \"…\", \"name\": \"…\", \"template\"?: \"init|workspace|blank\" }",
        );
    };
    let id = req.id.trim();
    let (dir, _) = match project_scope(store, id) {
        Ok(scope) => scope,
        Err(resp) => return resp,
    };
    let template = req
        .template
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("blank");
    let Some(content) = hook_template(template) else {
        return error(
            400,
            &format!("unknown template {template:?} (init | workspace | blank)"),
        );
    };
    if let Err(e) = ProjectHooks::new(&dir).create(req.name.trim(), content) {
        return Response::from(&e);
    }
    match build_workspaces_state(store, id) {
        Ok(state) => ok_json(&state),
        Err(resp) => resp,
    }
}

/// `POST /api/projects/workspaces/terminal/open` — ensure a tmux terminal session exists for
/// the workspace, started in its directory (idempotent — reopening attaches the view to the
/// live session), and reply with the first pane snapshot.
#[must_use]
pub fn open_workspace_terminal(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_workspace_term_ref(body) else {
        return bad_project_hook_ref();
    };
    let (id, name) = (req.id.trim(), req.name.trim());
    let entry = match resolve_workspace(store, id, name) {
        Ok(entry) => entry,
        Err(resp) => return resp,
    };
    if let Err(e) = terminal::open(id, name, &entry.path) {
        return Response::from(&e);
    }
    ok_json(&workspace_term(id, name))
}

/// `POST /api/projects/workspaces/terminal/peek` — a read-only snapshot of the workspace
/// terminal's pane, polled by the live view. A workspace without a live session answers
/// `running: false` (200, not an error).
#[must_use]
pub fn peek_workspace_terminal(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_workspace_term_ref(body) else {
        return bad_project_hook_ref();
    };
    let (id, name) = (req.id.trim(), req.name.trim());
    if let Err(resp) = resolve_workspace(store, id, name) {
        return resp;
    }
    ok_json(&workspace_term(id, name))
}

/// `POST /api/projects/workspaces/terminal/send` — type into the workspace terminal (`text`
/// literally, then the `key` tmux key name), replying with a fresh pane snapshot so the
/// keystrokes show without waiting for the next poll.
#[must_use]
pub fn send_workspace_terminal_keys(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_workspace_term_keys(body) else {
        return error(
            400,
            "expected JSON body { \"id\": \"…\", \"name\": \"…\", \"text\"?: \"…\", \"key\"?: \"…\" }",
        );
    };
    let (id, name) = (req.id.trim(), req.name.trim());
    if let Err(resp) = resolve_workspace(store, id, name) {
        return resp;
    }
    if let Err(e) = terminal::send_keys(id, name, &req.text, &req.key) {
        return Response::from(&e);
    }
    ok_json(&workspace_term(id, name))
}

/// `POST /api/projects/workspaces/terminal/kill` — kill the workspace's terminal session.
/// Idempotent: killing an already-gone terminal still answers the (now not-running) snapshot.
#[must_use]
pub fn kill_workspace_terminal(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_workspace_term_ref(body) else {
        return bad_project_hook_ref();
    };
    let (id, name) = (req.id.trim(), req.name.trim());
    if let Err(resp) = resolve_workspace(store, id, name) {
        return resp;
    }
    if let Err(e) = terminal::kill(id, name) {
        return Response::from(&e);
    }
    ok_json(&workspace_term(id, name))
}

/// The current [`WorkspaceTerm`] snapshot for a workspace (live-session flag, pane text,
/// attach command).
fn workspace_term(id: &str, name: &str) -> WorkspaceTerm {
    let running = terminal::is_running(id, name);
    WorkspaceTerm {
        id: id.to_string(),
        name: name.to_string(),
        running,
        output: terminal::capture(id, name).unwrap_or_default(),
        attach: format!("tmux attach -t {}", terminal::session_name(id, name)),
    }
}

/// Resolve a registered workspace entry by (project id, workspace name): the project gate
/// first (400/404 like everywhere else), then a 404 for an unknown workspace name.
fn resolve_workspace(
    store: &Projects,
    id: &str,
    name: &str,
) -> Result<adi_hooks::WorkspaceEntry, Response> {
    let (dir, _) = project_scope(store, id)?;
    let entries = Workspaces::new(&dir).list().map_err(|e| Response::from(&e))?;
    entries
        .into_iter()
        .find(|w| w.name == name)
        .ok_or_else(|| error(404, &format!("no such workspace: {name}")))
}

fn parse_workspace_term_ref(body: &[u8]) -> Option<WorkspaceTermRef> {
    let req: WorkspaceTermRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.name.trim().is_empty()).then_some(req)
}

fn parse_workspace_term_keys(body: &[u8]) -> Option<WorkspaceTermKeys> {
    let req: WorkspaceTermKeys = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.name.trim().is_empty()).then_some(req)
}

/// The full [`WorkspacesState`] for a registered project: entries decorated with live status,
/// hooks with their last-run status, and which lifecycle hook the next create would run.
fn build_workspaces_state(store: &Projects, id: &str) -> Result<WorkspacesState, Response> {
    let (dir, _) = project_scope(store, id)?;
    let ws = Workspaces::new(&dir);
    let hooks = ProjectHooks::new(&dir);

    let entries = ws.list().map_err(|e| Response::from(&e))?;
    let primary = entries
        .iter()
        .find(|w| w.kind != adi_hooks::WorkspaceKind::Local)
        .map(|w| w.name.clone());
    let workspaces = entries
        .iter()
        .map(|w| WorkspaceDto {
            name: w.name.clone(),
            path: w.path.display().to_string(),
            kind: w.kind.as_str().to_string(),
            status: ws.status(w).as_str().to_string(),
            pid: w.pid,
            hook: w.hook.clone(),
            created_at: w.created_at,
            primary: primary.as_deref() == Some(w.name.as_str()),
        })
        .collect();

    let hook_dtos = hooks
        .list()
        .map_err(|e| Response::from(&e))?
        .into_iter()
        .map(|h| {
            let status = hooks.status(&h.name);
            ProjectHookDto {
                last_run_at: hooks.last_run(&h.name),
                status: status.as_str().to_string(),
                exit_code: status.exit_code(),
                name: h.name,
                size: h.size,
                modified: h.modified,
            }
        })
        .collect();

    Ok(WorkspacesState {
        id: id.to_string(),
        next_hook: ws.next_hook().map_err(|e| Response::from(&e))?.to_string(),
        has_init_hook: hooks.exists(adi_hooks::HOOK_INIT),
        has_workspace_hook: hooks.exists(adi_hooks::HOOK_WORKSPACE),
        workspaces,
        hooks: hook_dtos,
    })
}

/// Resolve a *registered* project for the workspaces/hooks family: its directory plus the
/// `ADI_PROJECT_*` env pairs the hook contract needs. Same gate as [`project_jail`].
fn project_scope(
    store: &Projects,
    id: &str,
) -> Result<(PathBuf, Vec<(String, String)>), Response> {
    let project = match store.get(id) {
        Ok(Some(project)) => project,
        Ok(None) => return Err(error(404, &format!("no such project: {id}"))),
        Err(e) => return Err(Response::from(&e)),
    };
    let dir = store.project_dir(id).map_err(|e| Response::from(&e))?;
    let env = vec![
        ("ADI_PROJECT_ID".to_string(), project.id.clone()),
        (
            "ADI_PROJECT_NAME".to_string(),
            project.display_name().to_string(),
        ),
    ];
    Ok((dir, env))
}

// Map an adi-hooks error to an HTTP status. `Exists`/`NoHook`/`PrimaryMissing` are 409s —
// the request is well-formed but the project isn't in the right state, and the message
// says what to do about it.
impl From<&HookStoreError> for Response {
    fn from(e: &HookStoreError) -> Self {
        let status = match e {
            HookStoreError::InvalidName(_)
            | HookStoreError::EmptyHook(_)
            | HookStoreError::NotAbsolute(_)
            | HookStoreError::NotADir(_) => 400,
            HookStoreError::InvalidKey(_) => 400,
            HookStoreError::Exists(_)
            | HookStoreError::NoHook(_)
            | HookStoreError::PrimaryMissing
            | HookStoreError::NotRunning(_) => 409,
            HookStoreError::NotFound(_) => 404,
            HookStoreError::Launch(_)
            | HookStoreError::Tmux(_)
            | HookStoreError::Registry(_)
            | HookStoreError::Io(_) => 500,
        };
        error(status, &e.to_string())
    }
}

fn parse_workspaces_ref(body: &[u8]) -> Option<WorkspacesRef> {
    let req: WorkspacesRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty()).then_some(req)
}

fn parse_new_workspace(body: &[u8]) -> Option<NewWorkspace> {
    let req: NewWorkspace = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.name.trim().is_empty()).then_some(req)
}

fn parse_workspace_ref(body: &[u8]) -> Option<WorkspaceRef> {
    let req: WorkspaceRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.name.trim().is_empty()).then_some(req)
}

fn parse_project_hook_ref(body: &[u8]) -> Option<ProjectHookRef> {
    let req: ProjectHookRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.name.trim().is_empty()).then_some(req)
}

fn parse_new_project_hook(body: &[u8]) -> Option<NewProjectHook> {
    let req: NewProjectHook = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.name.trim().is_empty()).then_some(req)
}

fn bad_project_hook_ref() -> Response {
    error(400, "expected JSON body { \"id\": \"…\", \"name\": \"…\" }")
}

// MARK: hive — every service across all projects + the global front-door hive
