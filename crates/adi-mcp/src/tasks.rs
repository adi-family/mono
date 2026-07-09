//! The `tasks` feature: a small persistent task tracker agents use to record and update units
//! of work. State is one JSON document (`tasks.json`) under the `mcp` module dir of the shared
//! [`adi_config`] store, so tasks survive across agent sessions and processes. Each tool opens
//! the store fresh; writes are atomic (via the configurator's temp-then-rename).
//!
//! Tasks form a tree via each task's optional `parent`. Only three states are *stored*
//! (`open` / `done` / `archived`); a task's richer **effective** status (`ready` / `blocked` /
//! `done` / `archived`) is *computed* from that stored state plus its direct children, never
//! persisted — an open task is `blocked` while any direct child is still open, else `ready`.

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

/// A task's *stored* lifecycle state — the only status written to disk. Legacy names from the
/// previous model are accepted on read (via serde aliases) so old `tasks.json` files still load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TaskStatus {
    /// Not yet finished (covers legacy `pending` / `in_progress`).
    #[serde(rename = "open", alias = "pending", alias = "in_progress")]
    Open,
    /// Completed.
    Done,
    /// Abandoned / no longer relevant (covers legacy `cancelled`).
    #[serde(rename = "archived", alias = "cancelled")]
    Archived,
}

/// A task's *computed* status, derived from its stored status and direct children. Never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum EffectiveStatus {
    /// Open with no open direct child — actionable now.
    Ready,
    /// Open but waiting on at least one still-open direct child.
    Blocked,
    /// Stored status is `done`.
    Done,
    /// Stored status is `archived`.
    Archived,
}

/// One tracked unit of work (a node in the task tree).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Task {
    /// Stable id, assigned on creation (e.g. `t1`).
    id: String,
    /// Short one-line title.
    title: String,
    /// Optional longer details / notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    /// Stored lifecycle state.
    status: TaskStatus,
    /// Optional associated adi project id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    /// Optional parent task id — the link that forms the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    /// Optional free-form tag / label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
    /// Optional assignee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    assignee: Option<String>,
    /// Creation time (Unix epoch seconds).
    created_at: u64,
    /// Last-update time (Unix epoch seconds).
    updated_at: u64,
}

/// A task plus its derived fields — the shape every tool returns (never stored). Flattens all of
/// [`Task`]'s stored fields and adds the computed status and direct-child rollup.
#[derive(Debug, Serialize)]
struct TaskView {
    /// The stored task, inlined.
    #[serde(flatten)]
    task: Task,
    /// The computed status.
    effective: EffectiveStatus,
    /// Number of direct children.
    children_total: usize,
    /// Number of direct children whose stored status is `open`.
    children_open: usize,
}

impl TaskView {
    /// Build the view of `task` against the full task list `tasks` (needed for the tree rollup).
    fn of(task: Task, tasks: &[Task]) -> Self {
        let effective = effective_status(&task, tasks);
        let mut children_total = 0;
        let mut children_open = 0;
        for child in direct_children(tasks, &task.id) {
            children_total += 1;
            if child.status == TaskStatus::Open {
                children_open += 1;
            }
        }
        Self {
            task,
            effective,
            children_total,
            children_open,
        }
    }
}

/// The on-disk document: a monotonic id counter plus the task list.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TasksDoc {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    tasks: Vec<Task>,
}

/// A partial update to a task; `None` fields are left unchanged. Status is deliberately absent —
/// it moves only through the dedicated complete/archive/reopen tools.
#[derive(Debug, Default)]
struct TaskPatch {
    title: Option<String>,
    details: Option<String>,
    tag: Option<String>,
    assignee: Option<String>,
    /// A requested parent change: `None` leaves it; `Some("")` detaches to root; `Some(id)` sets.
    parent: Option<String>,
}

/// A resolved, validated parent change to apply to a task.
#[derive(Debug)]
enum ParentChange {
    /// Leave the parent as-is.
    Keep,
    /// Detach to root (no parent).
    Clear,
    /// Set the parent to this (existing, non-cycling) id.
    Set(String),
}

/// A failure from a [`TaskStore`] operation. The first four are *client* errors (the tools map
/// them to `invalid_params`); [`TaskError::Store`] wraps an internal I/O / (de)serialize failure.
#[derive(Debug)]
enum TaskError {
    /// No task with this id.
    NotFound(String),
    /// A referenced parent id does not exist.
    ParentMissing(String),
    /// Setting the requested parent would create a cycle.
    Cycle,
    /// Tried to complete an archived task.
    ReopenFirst,
    /// Underlying store I/O or (de)serialization failure.
    Store(anyhow::Error),
}

impl From<anyhow::Error> for TaskError {
    fn from(e: anyhow::Error) -> Self {
        TaskError::Store(e)
    }
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskError::NotFound(id) => write!(f, "no task with id {id:?}"),
            TaskError::ParentMissing(id) => write!(f, "no parent task with id {id:?}"),
            TaskError::Cycle => write!(f, "setting that parent would create a cycle"),
            TaskError::ReopenFirst => write!(f, "task is archived; reopen it first"),
            TaskError::Store(e) => write!(f, "task store error: {e}"),
        }
    }
}

impl std::error::Error for TaskError {}

/// A store result: `Ok` or a typed [`TaskError`] the tool layer maps to an [`McpError`].
type TaskResult<T> = Result<T, TaskError>;

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

    /// Create a new `open` task. If `parent` is given (and non-blank) it must already exist.
    fn create(
        &self,
        title: String,
        details: Option<String>,
        project: Option<String>,
        tag: Option<String>,
        parent: Option<String>,
    ) -> TaskResult<TaskView> {
        let mut doc = self.load()?;
        let parent = clean(parent);
        if let Some(pid) = parent.as_deref()
            && !doc.tasks.iter().any(|t| t.id == pid)
        {
            return Err(TaskError::ParentMissing(pid.to_string()));
        }
        doc.next_id += 1;
        let now = now_unix();
        let task = Task {
            id: format!("t{}", doc.next_id),
            title,
            details: clean(details),
            status: TaskStatus::Open,
            project: clean(project),
            parent,
            tag: clean(tag),
            assignee: None,
            created_at: now,
            updated_at: now,
        };
        let id = task.id.clone();
        doc.tasks.push(task);
        self.save(&doc)?;
        view_of(&doc, &id)
    }

    /// List task views, filtered by any provided stored/computed field.
    fn list(
        &self,
        project: Option<String>,
        tag: Option<String>,
        status: Option<TaskStatus>,
        effective: Option<EffectiveStatus>,
    ) -> TaskResult<Vec<TaskView>> {
        let doc = self.load()?;
        let project = clean(project);
        let tag = clean(tag);
        let views = doc
            .tasks
            .iter()
            .filter(|t| {
                status.is_none_or(|s| t.status == s)
                    && project
                        .as_deref()
                        .is_none_or(|p| t.project.as_deref() == Some(p))
                    && tag.as_deref().is_none_or(|g| t.tag.as_deref() == Some(g))
            })
            .map(|t| TaskView::of(t.clone(), &doc.tasks))
            .filter(|v| effective.is_none_or(|e| v.effective == e))
            .collect();
        Ok(views)
    }

    /// Get one task view by id.
    fn get(&self, id: &str) -> TaskResult<TaskView> {
        view_of(&self.load()?, id)
    }

    /// Edit a task's fields (never its status). Reparenting is validated against the tree.
    fn update(&self, id: &str, patch: TaskPatch) -> TaskResult<TaskView> {
        let mut doc = self.load()?;
        let Some(idx) = doc.tasks.iter().position(|t| t.id == id) else {
            return Err(TaskError::NotFound(id.to_string()));
        };
        // Resolve any parent change against the current tree before we take a mutable borrow.
        let parent_change = match patch.parent {
            None => ParentChange::Keep,
            Some(raw) => match clean(Some(raw)) {
                None => ParentChange::Clear,
                Some(pid) => {
                    if !doc.tasks.iter().any(|t| t.id == pid) {
                        return Err(TaskError::ParentMissing(pid));
                    }
                    if would_cycle(&doc.tasks, id, &pid) {
                        return Err(TaskError::Cycle);
                    }
                    ParentChange::Set(pid)
                }
            },
        };

        let task = &mut doc.tasks[idx];
        if let Some(title) = patch.title {
            task.title = title;
        }
        if let Some(details) = patch.details {
            task.details = clean(Some(details));
        }
        if let Some(tag) = patch.tag {
            task.tag = clean(Some(tag));
        }
        if let Some(assignee) = patch.assignee {
            task.assignee = clean(Some(assignee));
        }
        match parent_change {
            ParentChange::Keep => {}
            ParentChange::Clear => task.parent = None,
            ParentChange::Set(pid) => task.parent = Some(pid),
        }
        task.updated_at = now_unix();
        self.save(&doc)?;
        view_of(&doc, id)
    }

    /// Mark a task `done` (idempotent). An archived task must be reopened first. Open direct
    /// children do not block completion — the tool layer surfaces them as a warning.
    fn complete(&self, id: &str) -> TaskResult<TaskView> {
        let mut doc = self.load()?;
        let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
            return Err(TaskError::NotFound(id.to_string()));
        };
        if task.status == TaskStatus::Archived {
            return Err(TaskError::ReopenFirst);
        }
        task.status = TaskStatus::Done;
        task.updated_at = now_unix();
        self.save(&doc)?;
        view_of(&doc, id)
    }

    /// Archive a task. With `cascade`, also archive every still-open descendant (recursively);
    /// `done`/already-archived descendants are left untouched.
    fn archive(&self, id: &str, cascade: bool) -> TaskResult<TaskView> {
        let mut doc = self.load()?;
        if !doc.tasks.iter().any(|t| t.id == id) {
            return Err(TaskError::NotFound(id.to_string()));
        }
        let now = now_unix();
        let open_descendants = if cascade {
            descendants(&doc.tasks, id)
        } else {
            Vec::new()
        };
        for t in &mut doc.tasks {
            if t.id == id {
                if t.status != TaskStatus::Archived {
                    t.status = TaskStatus::Archived;
                    t.updated_at = now;
                }
            } else if t.status == TaskStatus::Open && open_descendants.contains(&t.id) {
                t.status = TaskStatus::Archived;
                t.updated_at = now;
            }
        }
        self.save(&doc)?;
        view_of(&doc, id)
    }

    /// Reopen a `done` or `archived` task back to `open` (idempotent on an already-open task).
    fn reopen(&self, id: &str) -> TaskResult<TaskView> {
        let mut doc = self.load()?;
        let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
            return Err(TaskError::NotFound(id.to_string()));
        };
        if task.status != TaskStatus::Open {
            task.status = TaskStatus::Open;
            task.updated_at = now_unix();
        }
        self.save(&doc)?;
        view_of(&doc, id)
    }

    /// Hard-delete a task, reparenting its direct children to the deleted task's parent so no
    /// dangling parent references remain.
    fn delete(&self, id: &str) -> TaskResult<()> {
        let mut doc = self.load()?;
        let Some(idx) = doc.tasks.iter().position(|t| t.id == id) else {
            return Err(TaskError::NotFound(id.to_string()));
        };
        let orphan_parent = doc.tasks[idx].parent.clone();
        for t in &mut doc.tasks {
            if t.parent.as_deref() == Some(id) {
                t.parent.clone_from(&orphan_parent);
            }
        }
        doc.tasks.remove(idx);
        self.save(&doc)?;
        Ok(())
    }
}

// ---- derivation (pure, over the full task list) ----------------------------------------

/// The direct children of `id` in `tasks` (those whose `parent` equals `id`).
fn direct_children<'a>(tasks: &'a [Task], id: &'a str) -> impl Iterator<Item = &'a Task> {
    tasks.iter().filter(move |t| t.parent.as_deref() == Some(id))
}

/// The computed status of `task` given the full task list: `archived`/`done` mirror the stored
/// status; an open task is `blocked` while any direct child is still open, else `ready`.
fn effective_status(task: &Task, tasks: &[Task]) -> EffectiveStatus {
    match task.status {
        TaskStatus::Archived => EffectiveStatus::Archived,
        TaskStatus::Done => EffectiveStatus::Done,
        TaskStatus::Open => {
            if direct_children(tasks, &task.id).any(|c| c.status == TaskStatus::Open) {
                EffectiveStatus::Blocked
            } else {
                EffectiveStatus::Ready
            }
        }
    }
}

/// All descendant ids of `root` (its whole subtree, excluding `root` itself).
fn descendants(tasks: &[Task], root: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_string()];
    while let Some(cur) = stack.pop() {
        for t in &direct_children(tasks, &cur).cloned().collect::<Vec<_>>() {
            out.push(t.id.clone());
            stack.push(t.id.clone());
        }
    }
    out
}

/// Whether making `new_parent` the parent of `task_id` would form a cycle — i.e. walking parent
/// links up from `new_parent` reaches `task_id` (which also rejects making a task its own parent).
fn would_cycle(tasks: &[Task], task_id: &str, new_parent: &str) -> bool {
    let mut cursor = Some(new_parent.to_string());
    while let Some(pid) = cursor {
        if pid == task_id {
            return true;
        }
        cursor = tasks.iter().find(|t| t.id == pid).and_then(|t| t.parent.clone());
    }
    false
}

/// Build the [`TaskView`] of `id` from `doc`, or [`TaskError::NotFound`] if it is gone.
fn view_of(doc: &TasksDoc, id: &str) -> TaskResult<TaskView> {
    match doc.tasks.iter().find(|t| t.id == id) {
        Some(task) => Ok(TaskView::of(task.clone(), &doc.tasks)),
        None => Err(TaskError::NotFound(id.to_string())),
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

/// Map a store [`TaskError`] to a fitting MCP error: the client errors become `invalid_params`,
/// a wrapped store failure becomes an `internal_error`.
// Consumed by value because it is used as a `.map_err(task_err)` adapter, which hands the error
// over by value.
#[allow(clippy::needless_pass_by_value)]
fn task_err(e: TaskError) -> McpError {
    match e {
        // A wrapped store failure is internal; every other variant is a client error whose
        // message is its `Display`.
        TaskError::Store(inner) => internal("task store error", inner),
        other => McpError::invalid_params(other.to_string(), None),
    }
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
    /// Optional free-form tag / label.
    #[serde(default)]
    tag: Option<String>,
    /// Optional parent task id; if given it must already exist (creates a subtask).
    #[serde(default)]
    parent: Option<String>,
}

/// Arguments for `tasks_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ListTasksArgs {
    /// Only return tasks associated with this project id.
    #[serde(default)]
    project: Option<String>,
    /// Only return tasks carrying this tag.
    #[serde(default)]
    tag: Option<String>,
    /// Only return tasks in this stored status (`open`, `done`, `archived`).
    #[serde(default)]
    status: Option<TaskStatus>,
    /// Only return tasks with this computed status (`ready`, `blocked`, `done`, `archived`).
    #[serde(default)]
    effective: Option<EffectiveStatus>,
}

/// Arguments for `tasks_get`, `tasks_complete`, `tasks_reopen`, and `tasks_delete`.
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
    /// New tag; pass an empty string to clear (unchanged if omitted).
    #[serde(default)]
    tag: Option<String>,
    /// New assignee; pass an empty string to clear (unchanged if omitted).
    #[serde(default)]
    assignee: Option<String>,
    /// New parent id; pass an empty string to detach to root. Must exist and must not create a
    /// cycle. Unchanged if omitted.
    #[serde(default)]
    parent: Option<String>,
}

/// Arguments for `tasks_archive`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArchiveTaskArgs {
    /// The task id to archive (e.g. `t1`).
    id: String,
    /// Also archive every still-open descendant (default: `true`).
    #[serde(default = "default_true")]
    cascade: bool,
}

/// The serde default for [`ArchiveTaskArgs::cascade`].
fn default_true() -> bool {
    true
}

/// The success shape of `tasks_complete` when the completed task still has open subtasks.
#[derive(Debug, Serialize)]
struct CompleteWithWarning {
    task: TaskView,
    warning: String,
}

#[tool_router(router = tasks_router, vis = "pub")]
impl AdiMcp {
    #[tool(description = "Create a task (stored status open) and return its view; a given parent must already exist")]
    async fn tasks_create(
        &self,
        Parameters(args): Parameters<CreateTaskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let title = args.title.trim().to_string();
        if title.is_empty() {
            return Err(McpError::invalid_params("title must not be empty", None));
        }
        let view = TaskStore::open()
            .create(title, args.details, args.project, args.tag, args.parent)
            .map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "List task views (with computed effective status), filtered by any of project, tag, stored status, or effective status"
    )]
    async fn tasks_list(
        &self,
        Parameters(args): Parameters<ListTasksArgs>,
    ) -> Result<CallToolResult, McpError> {
        let views = TaskStore::open()
            .list(args.project, args.tag, args.status, args.effective)
            .map_err(task_err)?;
        json_result(&views)
    }

    #[tool(description = "Get one task view by id, including its computed effective status")]
    async fn tasks_get(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = TaskStore::open().get(&args.id).map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "Edit a task's title, details, tag, assignee, or parent (not its status); only provided fields change. Empty string clears an optional field; reparenting must not create a cycle"
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
            tag: args.tag,
            assignee: args.assignee,
            parent: args.parent,
        };
        let view = TaskStore::open()
            .update(&args.id, patch)
            .map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "Mark a task done (idempotent). Completing a task that still has open subtasks succeeds but returns a warning. An archived task must be reopened first"
    )]
    async fn tasks_complete(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = TaskStore::open().complete(&args.id).map_err(task_err)?;
        if view.children_open >= 1 {
            let warning = format!(
                "task {} completed, but it still has {} open direct subtask(s)",
                view.task.id, view.children_open
            );
            return json_result(&CompleteWithWarning {
                task: view,
                warning,
            });
        }
        json_result(&view)
    }

    #[tool(
        description = "Archive a task; by default (cascade) also archives every still-open descendant"
    )]
    async fn tasks_archive(
        &self,
        Parameters(args): Parameters<ArchiveTaskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = TaskStore::open()
            .archive(&args.id, args.cascade)
            .map_err(task_err)?;
        json_result(&view)
    }

    #[tool(description = "Reopen a done or archived task, setting its stored status back to open")]
    async fn tasks_reopen(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = TaskStore::open().reopen(&args.id).map_err(task_err)?;
        json_result(&view)
    }

    #[tool(
        description = "Delete a task permanently; its direct children are reparented to the deleted task's parent"
    )]
    async fn tasks_delete(
        &self,
        Parameters(args): Parameters<TaskIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        TaskStore::open().delete(&args.id).map_err(task_err)?;
        Ok(text_result(format!("deleted task {}", args.id)))
    }
}

/// `adi-task` — a tiny command-line frontend over the *same* task store the MCP `tasks` feature
/// uses (`~/.adi/mono/mcp/tasks.json`). Shipped as its own binary so the task tree can be created
/// and driven by hand or from scripts, not only over MCP. A child module so it can reuse the
/// store types directly; only [`cli::run`] is re-exported (as `adi_mcp::run_tasks_cli`).
pub(crate) mod cli {
    use clap::{Parser, Subcommand};

    use super::{EffectiveStatus, TaskPatch, TaskStatus, TaskStore, TaskView};

    /// Manage the adi task tree from the shell (shares state with the adi-mcp `tasks` tools).
    #[derive(Debug, Parser)]
    #[command(name = "adi-task", version, about)]
    struct Cli {
        #[command(subcommand)]
        command: Command,
    }

    #[derive(Debug, Subcommand)]
    enum Command {
        /// Create a task (stored status `open`).
        Add {
            /// Short one-line title.
            title: String,
            /// Longer details / notes.
            #[arg(long)]
            details: Option<String>,
            /// Associated adi project id.
            #[arg(long)]
            project: Option<String>,
            /// Free-form tag / label.
            #[arg(long)]
            tag: Option<String>,
            /// Parent task id — makes this a subtask (the parent must already exist).
            #[arg(long)]
            parent: Option<String>,
        },
        /// List tasks with their computed effective status.
        List {
            /// Only tasks in this project.
            #[arg(long)]
            project: Option<String>,
            /// Only tasks carrying this tag.
            #[arg(long)]
            tag: Option<String>,
            /// Filter by stored status: `open` | `done` | `archived`.
            #[arg(long)]
            status: Option<String>,
            /// Filter by effective status: `ready` | `blocked` | `done` | `archived`.
            #[arg(long)]
            effective: Option<String>,
            /// Emit JSON instead of the text table.
            #[arg(long)]
            json: bool,
        },
        /// Show one task in full.
        Show {
            /// The task id (e.g. `t1`).
            id: String,
            /// Emit JSON.
            #[arg(long)]
            json: bool,
        },
        /// Mark a task done.
        Done {
            /// The task id.
            id: String,
        },
        /// Archive a task (and, by default, its still-open descendants).
        Archive {
            /// The task id.
            id: String,
            /// Archive only this task, not its open descendants.
            #[arg(long)]
            no_cascade: bool,
        },
        /// Reopen a done/archived task back to `open`.
        Reopen {
            /// The task id.
            id: String,
        },
        /// Edit a task's fields (not its status); empty string clears an optional field.
        Update {
            /// The task id.
            id: String,
            #[arg(long)]
            title: Option<String>,
            #[arg(long)]
            details: Option<String>,
            #[arg(long)]
            tag: Option<String>,
            #[arg(long)]
            assignee: Option<String>,
            /// New parent id; pass an empty string to detach to root.
            #[arg(long)]
            parent: Option<String>,
        },
        /// Delete a task permanently (its children reparent to its parent).
        Rm {
            /// The task id.
            id: String,
        },
    }

    /// Entry point for the `adi-task` binary.
    ///
    /// # Errors
    /// Propagates store failures — I/O, an unknown id, a missing parent, or a would-be cycle — as
    /// `anyhow` errors, plus a parse error for an invalid `--status`/`--effective` value.
    pub fn run() -> anyhow::Result<()> {
        let cli = Cli::parse();
        let store = TaskStore::open();
        match cli.command {
            Command::Add {
                title,
                details,
                project,
                tag,
                parent,
            } => print_line(&store.create(title, details, project, tag, parent)?),
            Command::List {
                project,
                tag,
                status,
                effective,
                json,
            } => {
                let status = status.as_deref().map(parse_status).transpose()?;
                let effective = effective.as_deref().map(parse_effective).transpose()?;
                let mut views = store.list(project, tag, status, effective)?;
                views.sort_by_key(|v| task_num(&v.task.id));
                if json {
                    println!("{}", serde_json::to_string_pretty(&views)?);
                } else if views.is_empty() {
                    println!("(no tasks)");
                } else {
                    for v in &views {
                        print_line(v);
                    }
                }
            }
            Command::Show { id, json } => {
                let view = store.get(&id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&view)?);
                } else {
                    print_show(&view);
                }
            }
            Command::Done { id } => {
                let view = store.complete(&id)?;
                print_line(&view);
                if view.children_open >= 1 {
                    eprintln!(
                        "warning: completed with {} still-open subtask(s)",
                        view.children_open
                    );
                }
            }
            Command::Archive { id, no_cascade } => print_line(&store.archive(&id, !no_cascade)?),
            Command::Reopen { id } => print_line(&store.reopen(&id)?),
            Command::Update {
                id,
                title,
                details,
                tag,
                assignee,
                parent,
            } => {
                let patch = TaskPatch {
                    title,
                    details,
                    tag,
                    assignee,
                    parent,
                };
                print_line(&store.update(&id, patch)?);
            }
            Command::Rm { id } => {
                store.delete(&id)?;
                println!("deleted {id}");
            }
        }
        Ok(())
    }

    /// Parse a stored-status filter.
    fn parse_status(s: &str) -> anyhow::Result<TaskStatus> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" => Ok(TaskStatus::Open),
            "done" => Ok(TaskStatus::Done),
            "archived" => Ok(TaskStatus::Archived),
            other => anyhow::bail!("invalid --status {other:?}; use open|done|archived"),
        }
    }

    /// Parse an effective-status filter.
    fn parse_effective(s: &str) -> anyhow::Result<EffectiveStatus> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ready" => Ok(EffectiveStatus::Ready),
            "blocked" => Ok(EffectiveStatus::Blocked),
            "done" => Ok(EffectiveStatus::Done),
            "archived" => Ok(EffectiveStatus::Archived),
            other => {
                anyhow::bail!("invalid --effective {other:?}; use ready|blocked|done|archived")
            }
        }
    }

    /// The numeric part of a `t<N>` id, for stable ordering (0 if it doesn't parse).
    fn task_num(id: &str) -> u64 {
        id.strip_prefix('t').and_then(|n| n.parse().ok()).unwrap_or(0)
    }

    /// The display label for a stored status.
    fn stored_label(s: TaskStatus) -> &'static str {
        match s {
            TaskStatus::Open => "open",
            TaskStatus::Done => "done",
            TaskStatus::Archived => "archived",
        }
    }

    /// The display label for a computed effective status.
    fn effective_label(e: EffectiveStatus) -> &'static str {
        match e {
            EffectiveStatus::Ready => "ready",
            EffectiveStatus::Blocked => "blocked",
            EffectiveStatus::Done => "done",
            EffectiveStatus::Archived => "archived",
        }
    }

    /// One compact line: `t1   [blocked ]  Title   (1/2 open)  @project  #tag`.
    fn print_line(v: &TaskView) {
        let children = if v.children_total > 0 {
            format!("  ({}/{} open)", v.children_open, v.children_total)
        } else {
            String::new()
        };
        let project = v.task.project.as_deref().map_or(String::new(), |p| format!("  @{p}"));
        let tag = v.task.tag.as_deref().map_or(String::new(), |t| format!("  #{t}"));
        println!(
            "{:<5} [{:<8}] {}{children}{project}{tag}",
            v.task.id,
            effective_label(v.effective),
            v.task.title
        );
    }

    /// The full task, one field per line.
    fn print_show(v: &TaskView) {
        println!("id:        {}", v.task.id);
        println!("title:     {}", v.task.title);
        println!("stored:    {}", stored_label(v.task.status));
        println!("effective: {}", effective_label(v.effective));
        if let Some(p) = &v.task.parent {
            println!("parent:    {p}");
        }
        println!("children:  {} ({} open)", v.children_total, v.children_open);
        if let Some(p) = &v.task.project {
            println!("project:   {p}");
        }
        if let Some(t) = &v.task.tag {
            println!("tag:       {t}");
        }
        if let Some(a) = &v.task.assignee {
            println!("assignee:  {a}");
        }
        if let Some(d) = &v.task.details {
            println!("details:   {d}");
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

    /// Convenience: create a child of `parent` (or a root task when `parent` is `None`).
    fn mk(store: &TaskStore, title: &str, parent: Option<&str>) -> TaskView {
        store
            .create(title.into(), None, None, None, parent.map(str::to_string))
            .expect("create")
    }

    #[test]
    fn create_assigns_incrementing_ids_and_defaults_to_open() {
        let store = scratch("create");
        let a = store.create("first".into(), None, None, None, None).expect("a");
        let b = store
            .create(
                "second".into(),
                Some("notes".into()),
                Some("demo".into()),
                Some("bug".into()),
                None,
            )
            .expect("b");
        assert_eq!(a.task.id, "t1");
        assert_eq!(b.task.id, "t2");
        assert_eq!(a.task.status, TaskStatus::Open);
        assert_eq!(a.effective, EffectiveStatus::Ready);
        assert_eq!(b.task.details.as_deref(), Some("notes"));
        assert_eq!(b.task.project.as_deref(), Some("demo"));
        assert_eq!(b.task.tag.as_deref(), Some("bug"));
    }

    #[test]
    fn create_with_parent_links_and_rejects_missing_parent() {
        let store = scratch("create-parent");
        let p = mk(&store, "parent", None);
        let c = mk(&store, "child", Some(&p.task.id));
        assert_eq!(c.task.parent.as_deref(), Some(p.task.id.as_str()));

        let err = store
            .create("orphan".into(), None, None, None, Some("t404".into()))
            .expect_err("missing parent");
        assert!(matches!(err, TaskError::ParentMissing(_)));
    }

    #[test]
    fn effective_status_derives_from_direct_children() {
        let store = scratch("derive");
        let p = mk(&store, "parent", None);
        let c = mk(&store, "child", Some(&p.task.id));

        // An open child blocks the parent.
        let pv = store.get(&p.task.id).expect("blocked");
        assert_eq!(pv.effective, EffectiveStatus::Blocked);
        assert_eq!(pv.children_total, 1);
        assert_eq!(pv.children_open, 1);

        // Completing the only child makes the parent ready.
        store.complete(&c.task.id).expect("complete child");
        let pv = store.get(&p.task.id).expect("ready");
        assert_eq!(pv.effective, EffectiveStatus::Ready);
        assert_eq!(pv.children_open, 0);
        assert_eq!(pv.children_total, 1);

        // A fresh open child blocks again; done/archived children never block.
        let c2 = mk(&store, "child2", Some(&p.task.id));
        assert_eq!(
            store.get(&p.task.id).unwrap().effective,
            EffectiveStatus::Blocked
        );
        store.archive(&c2.task.id, true).expect("archive child2");
        assert_eq!(
            store.get(&p.task.id).unwrap().effective,
            EffectiveStatus::Ready
        );
    }

    #[test]
    fn deep_tree_stays_blocked_until_middle_is_done() {
        let store = scratch("deep");
        let a = mk(&store, "A", None);
        let b = mk(&store, "B", Some(&a.task.id));
        let c = mk(&store, "C", Some(&b.task.id));

        assert_eq!(store.get(&a.task.id).unwrap().effective, EffectiveStatus::Blocked);
        assert_eq!(store.get(&b.task.id).unwrap().effective, EffectiveStatus::Blocked);

        // Completing the leaf frees B, but A is still blocked while B is open.
        store.complete(&c.task.id).unwrap();
        assert_eq!(store.get(&b.task.id).unwrap().effective, EffectiveStatus::Ready);
        assert_eq!(store.get(&a.task.id).unwrap().effective, EffectiveStatus::Blocked);

        // Only once B is done does A become ready.
        store.complete(&b.task.id).unwrap();
        assert_eq!(store.get(&a.task.id).unwrap().effective, EffectiveStatus::Ready);
    }

    #[test]
    fn complete_is_idempotent_and_reopen_restores_open() {
        let store = scratch("complete-reopen");
        let t = mk(&store, "t", None);

        let done = store.complete(&t.task.id).unwrap();
        assert_eq!(done.task.status, TaskStatus::Done);
        assert_eq!(done.effective, EffectiveStatus::Done);
        // Idempotent.
        assert_eq!(store.complete(&t.task.id).unwrap().task.status, TaskStatus::Done);

        let re = store.reopen(&t.task.id).unwrap();
        assert_eq!(re.task.status, TaskStatus::Open);
        assert_eq!(re.effective, EffectiveStatus::Ready);

        // Completing an archived task is refused until it is reopened.
        store.archive(&t.task.id, true).unwrap();
        assert!(matches!(store.complete(&t.task.id), Err(TaskError::ReopenFirst)));
        let re = store.reopen(&t.task.id).unwrap();
        assert_eq!(re.task.status, TaskStatus::Open);
    }

    #[test]
    fn complete_reports_open_children_via_rollup() {
        let store = scratch("complete-warn");
        let p = mk(&store, "parent", None);
        let _c = mk(&store, "child", Some(&p.task.id));
        // Completing a parent with an open child still succeeds; the view still counts the child.
        let done = store.complete(&p.task.id).unwrap();
        assert_eq!(done.task.status, TaskStatus::Done);
        assert_eq!(done.children_open, 1);
    }

    #[test]
    fn archive_cascade_only_archives_open_descendants() {
        let store = scratch("archive-cascade");
        let a = mk(&store, "A", None);
        let b = mk(&store, "B", Some(&a.task.id));
        let c = mk(&store, "C", Some(&b.task.id));
        // A done descendant should survive the cascade unchanged.
        store.complete(&c.task.id).unwrap();

        store.archive(&a.task.id, true).unwrap();
        assert_eq!(store.get(&a.task.id).unwrap().task.status, TaskStatus::Archived);
        assert_eq!(store.get(&b.task.id).unwrap().task.status, TaskStatus::Archived);
        assert_eq!(store.get(&c.task.id).unwrap().task.status, TaskStatus::Done);
    }

    #[test]
    fn archive_without_cascade_leaves_descendants() {
        let store = scratch("archive-nocascade");
        let a = mk(&store, "A", None);
        let b = mk(&store, "B", Some(&a.task.id));
        store.archive(&a.task.id, false).unwrap();
        assert_eq!(store.get(&a.task.id).unwrap().task.status, TaskStatus::Archived);
        assert_eq!(store.get(&b.task.id).unwrap().task.status, TaskStatus::Open);
    }

    #[test]
    fn update_edits_fields_and_reparenting_guards_cycles() {
        let store = scratch("update");
        let a = mk(&store, "A", None);
        let b = mk(&store, "B", Some(&a.task.id));

        // Plain field edits, with empty-string clearing.
        let up = store
            .update(
                &b.task.id,
                TaskPatch {
                    assignee: Some("alice".into()),
                    tag: Some("bug".into()),
                    ..TaskPatch::default()
                },
            )
            .unwrap();
        assert_eq!(up.task.assignee.as_deref(), Some("alice"));
        assert_eq!(up.task.tag.as_deref(), Some("bug"));
        assert_eq!(up.task.title, "B");
        let cleared = store
            .update(
                &b.task.id,
                TaskPatch {
                    tag: Some(String::new()),
                    ..TaskPatch::default()
                },
            )
            .unwrap();
        assert_eq!(cleared.task.tag, None);

        // Reparent guards: self, descendant-cycle, missing parent.
        assert!(matches!(
            store.update(
                &a.task.id,
                TaskPatch { parent: Some(a.task.id.clone()), ..TaskPatch::default() }
            ),
            Err(TaskError::Cycle)
        ));
        assert!(matches!(
            store.update(
                &a.task.id,
                TaskPatch { parent: Some(b.task.id.clone()), ..TaskPatch::default() }
            ),
            Err(TaskError::Cycle)
        ));
        assert!(matches!(
            store.update(
                &a.task.id,
                TaskPatch { parent: Some("t404".into()), ..TaskPatch::default() }
            ),
            Err(TaskError::ParentMissing(_))
        ));

        // Clearing the parent detaches to root.
        let detached = store
            .update(
                &b.task.id,
                TaskPatch { parent: Some(String::new()), ..TaskPatch::default() },
            )
            .unwrap();
        assert_eq!(detached.task.parent, None);

        // Missing id.
        assert!(matches!(
            store.update("nope", TaskPatch::default()),
            Err(TaskError::NotFound(_))
        ));
    }

    #[test]
    fn delete_reparents_children_to_the_deleted_parent() {
        let store = scratch("delete");
        let a = mk(&store, "A", None);
        let b = mk(&store, "B", Some(&a.task.id));
        let c = mk(&store, "C", Some(&b.task.id));

        store.delete(&b.task.id).unwrap();
        assert!(matches!(store.get(&b.task.id), Err(TaskError::NotFound(_))));
        // C is reparented from B up to A.
        assert_eq!(
            store.get(&c.task.id).unwrap().task.parent.as_deref(),
            Some(a.task.id.as_str())
        );
        assert!(matches!(store.delete("t404"), Err(TaskError::NotFound(_))));
    }

    #[test]
    fn list_filters_by_project_tag_status_and_effective() {
        let store = scratch("list");
        let _a = store
            .create("a".into(), None, Some("p1".into()), Some("x".into()), None)
            .unwrap();
        let b = store
            .create("b".into(), None, Some("p2".into()), Some("y".into()), None)
            .unwrap();
        store.complete(&b.task.id).unwrap();

        assert_eq!(store.list(None, None, None, None).unwrap().len(), 2);
        assert_eq!(store.list(None, None, Some(TaskStatus::Done), None).unwrap().len(), 1);
        assert_eq!(store.list(Some("p1".into()), None, None, None).unwrap().len(), 1);
        assert_eq!(store.list(None, Some("x".into()), None, None).unwrap().len(), 1);
        assert_eq!(
            store.list(None, None, None, Some(EffectiveStatus::Done)).unwrap().len(),
            1
        );
        assert_eq!(
            store.list(None, None, None, Some(EffectiveStatus::Ready)).unwrap().len(),
            1
        );
    }

    #[test]
    fn legacy_document_migrates_stored_statuses_and_defaults_new_fields() {
        let store = scratch("legacy");
        let legacy = r#"{
            "next_id": 3,
            "tasks": [
                {"id":"t1","title":"a","status":"pending","created_at":1,"updated_at":1},
                {"id":"t2","title":"b","status":"in_progress","created_at":1,"updated_at":1},
                {"id":"t3","title":"c","status":"cancelled","created_at":1,"updated_at":1}
            ]
        }"#;
        store
            .module
            .write_raw(TASKS_FILE, legacy.as_bytes())
            .expect("seed legacy doc");

        let t1 = store.get("t1").expect("t1");
        let t2 = store.get("t2").expect("t2");
        let t3 = store.get("t3").expect("t3");
        assert_eq!(t1.task.status, TaskStatus::Open);
        assert_eq!(t2.task.status, TaskStatus::Open);
        assert_eq!(t3.task.status, TaskStatus::Archived);
        // New fields absent in the legacy blob default to None.
        assert_eq!(t1.task.parent, None);
        assert_eq!(t1.task.tag, None);
        assert_eq!(t1.task.assignee, None);
    }
}
