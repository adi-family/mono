use adi_tasks::Error as TaskStoreError;
use adi_tasks::TaskView;
use adi_tasks::Tasks;

use crate::types::{CompleteTask, GiveUpTask, NewTask, TaskRef, TaskRow, TasksState};

use super::response::{FromBody, Response, error, mutate, ok_json};

/// `GET /api/tasks` — the whole task tree as a flat list, ordered by task number so a parent
/// precedes the children created after it. The client nests them into a tree by `parent`.
#[must_use]
pub fn tasks(store: &Tasks) -> Response {
    match store.list(None, None, None, None) {
        Ok(mut views) => {
            views.sort_by(|a, b| a.order(b));
            ok_json(&TasksState {
                tasks: views.iter().map(task_row).collect(),
            })
        }
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/tasks/create` — create a task (stored status `open`), then report the fresh tree.
/// Only `title` is required; a given `parent` must be an existing task id.
#[must_use]
pub fn create_task(store: &Tasks, body: &[u8]) -> Response {
    mutate(
        body,
        |req: NewTask| {
            store.create(
                req.title.trim().to_string(),
                req.details,
                req.project,
                req.tag,
                req.parent,
                req.cwd,
            )
        },
        || tasks(store),
    )
}

/// `POST /api/tasks/archive` — archive a task (stored status `archived`), then report the fresh
/// tree. With `cascade`, its open descendants are archived along with it; without, they stay open
/// and re-root into the live tree.
#[must_use]
pub fn archive_task(store: &Tasks, body: &[u8]) -> Response {
    mutate(
        body,
        |req: TaskRef| store.archive(req.id.trim(), req.cascade),
        || tasks(store),
    )
}

/// `POST /api/tasks/reopen` — return a done or archived task to `open`, then report the fresh
/// tree. This is the undo for archive.
#[must_use]
pub fn reopen_task(store: &Tasks, body: &[u8]) -> Response {
    mutate(
        body,
        |req: TaskRef| store.reopen(req.id.trim()),
        || tasks(store),
    )
}

/// `POST /api/tasks/delete` — permanently remove a task, reparenting its direct children to the
/// deleted task's parent so no dangling links remain, then report the fresh tree. Irreversible —
/// the UI gates it behind a confirm. The body's `cascade` is ignored (children are kept, reparented).
#[must_use]
pub fn delete_task(store: &Tasks, body: &[u8]) -> Response {
    mutate(
        body,
        |req: TaskRef| store.delete(req.id.trim()),
        || tasks(store),
    )
}

/// `POST /api/tasks/complete` — spec section F: the panel's own form, rendered from a task's
/// `output_schema`, finishes a task exactly like `adi-mono tasks complete --result` does: the same
/// store call, the same validation, the same refusal naming the exact path that didn't fit.
#[must_use]
pub fn complete_task(store: &Tasks, body: &[u8]) -> Response {
    mutate(
        body,
        |req: CompleteTask| store.complete(req.id.trim(), req.result),
        || tasks(store),
    )
}

/// `POST /api/tasks/give-up` — the panel's form of `adi-mono tasks give-up --reason`. Stores a
/// fact (`give_up_reason`); the task's stored `status` is left exactly as it was.
#[must_use]
pub fn give_up_task(store: &Tasks, body: &[u8]) -> Response {
    mutate(
        body,
        |req: GiveUpTask| store.give_up(req.id.trim(), req.reason.trim()),
        || tasks(store),
    )
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
        cwd: task.cwd.clone(),
        parent: task.parent.clone(),
        tag: task.tag.clone(),
        assignee: task.assignee.clone(),
        children_total: view.children_total,
        children_open: view.children_open,
        created_at: task.created_at,
        updated_at: task.updated_at,
        input: task.input.clone(),
        output_schema: task.output_schema.clone(),
        result: task.result.clone(),
        give_up_reason: task.give_up_reason.clone(),
        workflow_run: task.workflow_run.clone(),
    }
}

// Map a task-store error to an HTTP status: missing → 404, bad edit → 400, conflict → 409, else
// 500.
impl From<&TaskStoreError> for Response {
    fn from(e: &TaskStoreError) -> Self {
        let status = match e {
            TaskStoreError::NotFound(_) => 404,
            TaskStoreError::ParentMissing(_)
            | TaskStoreError::Cycle
            | TaskStoreError::InvalidOutputSchema(_)
            | TaskStoreError::ResultRequired
            | TaskStoreError::ResultInvalid { .. } => 400,
            TaskStoreError::ReopenFirst
            | TaskStoreError::DuplicateStepId(_)
            | TaskStoreError::NotOpen(_) => 409,
            TaskStoreError::Store(_) => 500,
        };
        error(status, &e.to_string())
    }
}

impl FromBody for NewTask {
    const EXPECTED: &'static str = "expected JSON body { \"title\": \"…\" } with a non-empty title";

    fn is_complete(&self) -> bool {
        !self.title.trim().is_empty()
    }
}

impl FromBody for TaskRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" } with a non-empty task id";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for CompleteTask {
    const EXPECTED: &'static str =
        "expected JSON body { \"id\": \"…\", \"result\"?: <json> } with a non-empty task id";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for GiveUpTask {
    const EXPECTED: &'static str =
        "expected JSON body { \"id\": \"…\", \"reason\": \"…\" } with both non-empty";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty() && !self.reason.trim().is_empty()
    }
}

// MARK: agents — AgentDef definitions under ~/.adi/mono/agents
