use adi_tasks::Error as TaskStoreError;
use adi_tasks::TaskView;
use adi_tasks::Tasks;

use crate::types::{NewTask, TaskRow, TasksState};

use super::response::{error, ok_json};

/// `GET /api/tasks` — the whole task tree as a flat list, ordered by task number so a parent
/// precedes the children created after it. The client nests them into a tree by `parent`.
#[must_use]
pub fn tasks(store: &Tasks) -> (u16, String) {
    match store.list(None, None, None, None) {
        Ok(mut views) => {
            views.sort_by(|a, b| a.order(b));
            ok_json(&TasksState {
                tasks: views.iter().map(task_row).collect(),
            })
        }
        Err(e) => task_error(&e),
    }
}

/// `POST /api/tasks/create` — create a task (stored status `open`), then report the fresh tree.
/// Only `title` is required; a given `parent` must be an existing task id.
#[must_use]
pub fn create_task(store: &Tasks, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_new_task(body) else {
        return bad_new_task();
    };
    match store.create(
        req.title.trim().to_string(),
        req.details,
        req.project,
        req.tag,
        req.parent,
    ) {
        Ok(_) => tasks(store),
        Err(e) => task_error(&e),
    }
}

/// Flatten a store [`TaskView`] into its wire [`TaskRow`] DTO, stringifying the status enums.
fn task_row(view: &TaskView) -> TaskRow {
    let task = &view.task;
    TaskRow {
        id: task.id.clone(),
        title: task.title.clone(),
        details: task.details.clone(),
        status: task.status.as_str().to_string(),
        effective: view.effective.as_str().to_string(),
        project: task.project.clone(),
        parent: task.parent.clone(),
        tag: task.tag.clone(),
        assignee: task.assignee.clone(),
        children_total: view.children_total,
        children_open: view.children_open,
        created_at: task.created_at,
        updated_at: task.updated_at,
    }
}

/// Map a task-store error to an HTTP status: missing → 404, bad edit → 400, archived → 409, else 500.
fn task_error(e: &TaskStoreError) -> (u16, String) {
    let status = match e {
        TaskStoreError::NotFound(_) => 404,
        TaskStoreError::ParentMissing(_) | TaskStoreError::Cycle => 400,
        TaskStoreError::ReopenFirst => 409,
        TaskStoreError::Store(_) => 500,
    };
    error(status, &e.to_string())
}

fn parse_new_task(body: &[u8]) -> Option<NewTask> {
    let req: NewTask = serde_json::from_slice(body).ok()?;
    (!req.title.trim().is_empty()).then_some(req)
}

fn bad_new_task() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"title\": \"…\" } with a non-empty title",
    )
}

// MARK: agents — AgentDef definitions under ~/.adi/mono/agents
