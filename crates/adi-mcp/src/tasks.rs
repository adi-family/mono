//! The `tasks` feature: a small persistent task tracker agents use to record and update
//! units of work. State is one JSON document (`tasks.json`) under the `mcp` module dir of the
//! shared [`adi_config`] store, so tasks survive across agent sessions and processes. Each
//! tool opens the store fresh; writes are atomic (via the configurator's temp-then-rename).

use std::time::{SystemTime, UNIX_EPOCH};

use adi_config::{Config, Module};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::server::{AdiMcp, internal, json_result, text_result};

/// The config module (`~/.adi/mono/mcp`) the tracker persists under.
const MODULE: &str = "mcp";
/// The tracker's on-disk document.
const TASKS_FILE: &str = "tasks.json";

/// A task's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TaskStatus {
    /// Not started.
    Pending,
    /// Actively being worked on.
    InProgress,
    /// Completed.
    Done,
    /// Abandoned / no longer relevant.
    Cancelled,
}

/// One tracked unit of work.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct Task {
    /// Stable id, assigned on creation (e.g. `t1`).
    id: String,
    /// Short one-line title.
    title: String,
    /// Optional longer details / notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    /// Lifecycle state.
    status: TaskStatus,
    /// Optional associated adi project id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    /// Creation time (Unix epoch seconds).
    created_at: u64,
    /// Last-update time (Unix epoch seconds).
    updated_at: u64,
}

/// The on-disk document: a monotonic id counter plus the task list.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TasksDoc {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    tasks: Vec<Task>,
}

/// A partial update to a task; `None` fields are left unchanged.
#[derive(Debug, Default)]
struct TaskPatch {
    title: Option<String>,
    details: Option<String>,
    status: Option<TaskStatus>,
    project: Option<String>,
}

/// Reads and writes the tasks document under the `mcp` module dir.
#[derive(Debug)]
struct TaskStore {
    module: Module,
}

impl TaskStore {
    /// Open the store backed by the standard config store (`~/.adi/mono/mcp`).
    fn open() -> Self {
        Self::with_config(&Config::open())
    }

    /// Open the store backed by a caller-supplied config (tests / alternate installs).
    fn with_config(config: &Config) -> Self {
        Self {
            module: config.module(MODULE),
        }
    }

    fn load(&self) -> anyhow::Result<TasksDoc> {
        match self.module.read_raw(TASKS_FILE)? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)?),
            None => Ok(TasksDoc::default()),
        }
    }

    fn save(&self, doc: &TasksDoc) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(doc)?;
        self.module.write_raw(TASKS_FILE, &bytes)?;
        Ok(())
    }

    fn create(
        &self,
        title: String,
        details: Option<String>,
        project: Option<String>,
    ) -> anyhow::Result<Task> {
        let mut doc = self.load()?;
        doc.next_id += 1;
        let now = now_unix();
        let task = Task {
            id: format!("t{}", doc.next_id),
            title,
            details: clean(details),
            status: TaskStatus::Pending,
            project: clean(project),
            created_at: now,
            updated_at: now,
        };
        doc.tasks.push(task.clone());
        self.save(&doc)?;
        Ok(task)
    }

    fn list(
        &self,
        status: Option<TaskStatus>,
        project: Option<String>,
    ) -> anyhow::Result<Vec<Task>> {
        let doc = self.load()?;
        let project = clean(project);
        Ok(doc
            .tasks
            .into_iter()
            .filter(|t| {
                status.is_none_or(|s| t.status == s)
                    && project
                        .as_deref()
                        .is_none_or(|p| t.project.as_deref() == Some(p))
            })
            .collect())
    }

    fn get(&self, id: &str) -> anyhow::Result<Option<Task>> {
        Ok(self.load()?.tasks.into_iter().find(|t| t.id == id))
    }

    fn update(&self, id: &str, patch: TaskPatch) -> anyhow::Result<Option<Task>> {
        let mut doc = self.load()?;
        let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
            return Ok(None);
        };
        if let Some(title) = patch.title {
            task.title = title;
        }
        if let Some(details) = patch.details {
            task.details = clean(Some(details));
        }
        if let Some(status) = patch.status {
            task.status = status;
        }
        if let Some(project) = patch.project {
            task.project = clean(Some(project));
        }
        task.updated_at = now_unix();
        let updated = task.clone();
        self.save(&doc)?;
        Ok(Some(updated))
    }

    fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let mut doc = self.load()?;
        let before = doc.tasks.len();
        doc.tasks.retain(|t| t.id != id);
        let removed = doc.tasks.len() != before;
        if removed {
            self.save(&doc)?;
        }
        Ok(removed)
    }
}

/// The current time as Unix epoch seconds (0 if the clock predates the epoch).
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Trim a string, dropping it entirely when blank (so `""` clears an optional field).
fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// A "task not found" client error, shared by the get/update/delete tools.
fn not_found(id: &str) -> McpError {
    McpError::invalid_params(format!("no task with id {id:?}"), None)
}

// ---- MCP tools -------------------------------------------------------------------------

/// Arguments for `tasks_create`.
#[derive(Debug, Deserialize, JsonSchema)]
struct CreateTaskArgs {
    /// Short one-line title for the task (required).
    title: String,
    /// Optional longer details or notes.
    #[serde(default)]
    details: Option<String>,
    /// Optional adi project id to associate the task with.
    #[serde(default)]
    project: Option<String>,
}

/// Arguments for `tasks_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ListTasksArgs {
    /// Only return tasks in this status (`pending`, `in_progress`, `done`, `cancelled`).
    #[serde(default)]
    status: Option<TaskStatus>,
    /// Only return tasks associated with this project id.
    #[serde(default)]
    project: Option<String>,
}

/// Arguments for `tasks_get` and `tasks_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct TaskIdArgs {
    /// The task id (e.g. `t1`).
    id: String,
}

/// Arguments for `tasks_update`. Only the fields you pass change; omit a field to leave it.
#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateTaskArgs {
    /// The task id to update (e.g. `t1`).
    id: String,
    /// New title (unchanged if omitted; must not be blank if given).
    #[serde(default)]
    title: Option<String>,
    /// New details; pass an empty string to clear (unchanged if omitted).
    #[serde(default)]
    details: Option<String>,
    /// New status (unchanged if omitted).
    #[serde(default)]
    status: Option<TaskStatus>,
    /// New associated project id; empty string clears (unchanged if omitted).
    #[serde(default)]
    project: Option<String>,
}

#[tool_router(router = tasks_router, vis = "pub")]
impl AdiMcp {
    #[tool(description = "Create a task in the agent task tracker and return it")]
    async fn tasks_create(
        &self,
        Parameters(args): Parameters<CreateTaskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let title = args.title.trim().to_string();
        if title.is_empty() {
            return Err(McpError::invalid_params("title must not be empty", None));
        }
        let task = TaskStore::open()
            .create(title, args.details, args.project)
            .map_err(|e| internal("failed to create task", e))?;
        json_result(&task)
    }

    #[tool(description = "List tasks, optionally filtered by status and/or project")]
    async fn tasks_list(
        &self,
        Parameters(args): Parameters<ListTasksArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tasks = TaskStore::open()
            .list(args.status, args.project)
            .map_err(|e| internal("failed to list tasks", e))?;
        json_result(&tasks)
    }

    #[tool(description = "Get a single task by id")]
    async fn tasks_get(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        match TaskStore::open()
            .get(&args.id)
            .map_err(|e| internal("failed to read task", e))?
        {
            Some(task) => json_result(&task),
            None => Err(not_found(&args.id)),
        }
    }

    #[tool(
        description = "Update a task's title, details, status, or project; only provided fields change"
    )]
    async fn tasks_update(
        &self,
        Parameters(args): Parameters<UpdateTaskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let title = args.title.map(|t| t.trim().to_string());
        if matches!(&title, Some(t) if t.is_empty()) {
            return Err(McpError::invalid_params("title must not be empty", None));
        }
        let patch = TaskPatch {
            title,
            details: args.details,
            status: args.status,
            project: args.project,
        };
        match TaskStore::open()
            .update(&args.id, patch)
            .map_err(|e| internal("failed to update task", e))?
        {
            Some(task) => json_result(&task),
            None => Err(not_found(&args.id)),
        }
    }

    #[tool(description = "Delete a task by id")]
    async fn tasks_delete(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let removed = TaskStore::open()
            .delete(&args.id)
            .map_err(|e| internal("failed to delete task", e))?;
        if removed {
            Ok(text_result(format!("deleted task {}", args.id)))
        } else {
            Err(not_found(&args.id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> TaskStore {
        let root = std::env::temp_dir().join(format!(
            "adi-mcp-tasks-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        TaskStore::with_config(&Config::with_root(root))
    }

    #[test]
    fn create_assigns_incrementing_ids_and_defaults_to_pending() {
        let store = scratch("create");
        let a = store.create("first".into(), None, None).expect("create a");
        let b = store
            .create("second".into(), Some("notes".into()), Some("demo".into()))
            .expect("create b");
        assert_eq!(a.id, "t1");
        assert_eq!(b.id, "t2");
        assert_eq!(a.status, TaskStatus::Pending);
        assert_eq!(b.details.as_deref(), Some("notes"));
        assert_eq!(b.project.as_deref(), Some("demo"));
    }

    #[test]
    fn list_filters_by_status_and_project() {
        let store = scratch("list");
        store.create("a".into(), None, Some("p1".into())).expect("a");
        let b = store.create("b".into(), None, Some("p2".into())).expect("b");
        store
            .update(
                &b.id,
                TaskPatch {
                    status: Some(TaskStatus::Done),
                    ..TaskPatch::default()
                },
            )
            .expect("update b");

        assert_eq!(store.list(None, None).expect("all").len(), 2);
        assert_eq!(store.list(Some(TaskStatus::Done), None).expect("done").len(), 1);
        assert_eq!(
            store.list(None, Some("p1".into())).expect("p1").len(),
            1
        );
        assert!(store.list(Some(TaskStatus::Pending), Some("p2".into())).expect("none").is_empty());
    }

    #[test]
    fn update_patches_only_given_fields_and_reports_missing() {
        let store = scratch("update");
        let t = store.create("title".into(), Some("d".into()), None).expect("create");
        let updated = store
            .update(
                &t.id,
                TaskPatch {
                    status: Some(TaskStatus::InProgress),
                    ..TaskPatch::default()
                },
            )
            .expect("update")
            .expect("present");
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.title, "title");
        assert_eq!(updated.details.as_deref(), Some("d"));

        // Empty-string details clears the field.
        let cleared = store
            .update(
                &t.id,
                TaskPatch {
                    details: Some(String::new()),
                    ..TaskPatch::default()
                },
            )
            .expect("clear")
            .expect("present");
        assert_eq!(cleared.details, None);

        assert!(store.update("nope", TaskPatch::default()).expect("missing").is_none());
    }

    #[test]
    fn delete_removes_and_is_idempotent() {
        let store = scratch("delete");
        let t = store.create("x".into(), None, None).expect("create");
        assert!(store.delete(&t.id).expect("delete"));
        assert!(store.get(&t.id).expect("get").is_none());
        assert!(!store.delete(&t.id).expect("delete missing"));
    }
}
