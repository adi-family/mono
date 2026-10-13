//! The task tree's data types — the stored [`Task`], the derived [`TaskView`], the two status
//! enums, a partial-update [`TaskPatch`] — plus the pure derivation helpers that compute a
//! task's effective status and validate tree edits. None of this touches disk; the [`Tasks`]
//! store in [`crate`] owns I/O.

pub(crate) use adi_config::clean;
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A task's *stored* lifecycle state — the only status written to disk. Legacy names from the
/// previous model are accepted on read (via serde aliases) so old `tasks.json` files still load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Not yet finished (covers legacy `pending` / `in_progress`).
    #[serde(rename = "open", alias = "pending", alias = "in_progress")]
    Open,
    /// Completed.
    Done,
    /// Abandoned / no longer relevant (covers legacy `cancelled`).
    #[serde(rename = "archived", alias = "cancelled")]
    Archived,
}

impl TaskStatus {
    /// The stable wire/CLI label for this stored status (matches the serde representation).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Open => "open",
            TaskStatus::Done => "done",
            TaskStatus::Archived => "archived",
        }
    }

    /// The inverse of [`as_str`](Self::as_str), for the one column in the store that isn't typed
    /// SQL — a label this build doesn't recognise (written by a newer one, or hand-edited) reads
    /// as `open` rather than failing the row, the same leniency the legacy serde aliases give a
    /// `tasks.json` written by the previous model.
    #[must_use]
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "done" => TaskStatus::Done,
            "archived" | "cancelled" => TaskStatus::Archived,
            _ => TaskStatus::Open,
        }
    }
}

/// A task's *computed* status, derived from its stored status and direct children. Never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveStatus {
    /// Open with no open direct child — actionable now.
    Ready,
    /// Open but waiting on at least one still-open direct child.
    Blocked,
    /// Stored status is `done`.
    Done,
    /// Stored status is `archived`.
    Archived,
}

impl EffectiveStatus {
    /// The stable wire/CLI label for this computed status (matches the serde representation).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EffectiveStatus::Ready => "ready",
            EffectiveStatus::Blocked => "blocked",
            EffectiveStatus::Done => "done",
            EffectiveStatus::Archived => "archived",
        }
    }
}

/// One tracked unit of work (a node in the task tree).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    /// Stable id, assigned on creation (e.g. `t1`).
    pub id: String,
    /// Short one-line title.
    pub title: String,
    /// Optional longer details / notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// Stored lifecycle state.
    pub status: TaskStatus,
    /// Optional associated adi project id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Where this task's work happens — the directory a run picking it up should start in
    /// (`~` allowed). A task is often *about* one place: a repo, a workspace clone, a
    /// dashboard's directory. Saying so once, on the task, is what lets whatever dispatches it
    /// hand that directory to the run (`working_dir` of `POST /api/agents/run`) instead of
    /// describing the path in prose and leaving each command to `cd` there again.
    ///
    /// `None` means the task has no opinion: the run starts wherever the agent is defined to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Optional parent task id — the link that forms the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Optional free-form tag / label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Optional assignee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Creation time (Unix epoch seconds).
    pub created_at: u64,
    /// Last-update time (Unix epoch seconds).
    pub updated_at: u64,
    /// The input a run picking up this task was given — set once, at creation, by whatever filed
    /// it (a workflow's `ctx.task`, most often). `None` for an ordinary task nobody handed
    /// structured input: its instructions are [`details`](Self::details) alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// The JSON Schema a `--result` must satisfy to [`complete`](crate::Tasks::complete) this
    /// task. `None` means any result (or none) is accepted — `complete` requires nothing more
    /// than the task existing, same as before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    /// The value a run completed this task with, once validated against
    /// [`output_schema`](Self::output_schema). `None` until completion, and for a task completed
    /// before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// The workflow run this task is a step of, or `None` for a task nobody's workflow owns.
    /// Paired with [`step_key`](Self::step_key) to form [`step_id`](Self::step_id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run: Option<String>,
    /// This task's key within [`workflow_run`](Self::workflow_run) — unique within that run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_key: Option<String>,
    /// Why an agent gave up on this task, if one has. A stored *fact*, not a status: giving up
    /// leaves [`status`](Self::status) exactly as it was (still `open`, almost always) so a
    /// dispatcher can compute "stuck" from it rather than this field doubling as one more lifecycle
    /// state. Cleared by [`reopen`](crate::Tasks::reopen) and [`complete`](crate::Tasks::complete)
    /// — both leave the "waiting on a decision about this give-up" state behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub give_up_reason: Option<String>,
}

impl Task {
    /// The unique id a workflow step is addressed by: `${workflow_run}/${step_key}`, or `None`
    /// when either half is absent — an ordinary task has no stepId at all, not an empty one.
    #[must_use]
    pub fn step_id(&self) -> Option<String> {
        match (&self.workflow_run, &self.step_key) {
            (Some(run), Some(key)) => Some(format!("{run}/{key}")),
            _ => None,
        }
    }
}

/// A task plus its derived fields — the shape every store method returns (never stored). Flattens
/// all of [`Task`]'s stored fields and adds the computed status and direct-child rollup.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskView {
    /// The stored task, inlined.
    #[serde(flatten)]
    pub task: Task,
    /// The computed status.
    pub effective: EffectiveStatus,
    /// Number of direct children.
    pub children_total: usize,
    /// Number of direct children whose stored status is `open`.
    pub children_open: usize,
}

impl TaskView {
    /// Build the view of `task` against the full task list `tasks` (needed for the tree rollup).
    pub(crate) fn of(task: Task, tasks: &[Task]) -> Self {
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

    /// Stable task ordering: by creation time (so a parent precedes children created after it),
    /// with the id as a tiebreak. Works across both id schemes (`t<N>` and Jira `<KEY>-<N>`).
    #[must_use]
    pub fn order(&self, other: &TaskView) -> std::cmp::Ordering {
        self.task
            .created_at
            .cmp(&other.task.created_at)
            .then_with(|| self.task.id.cmp(&other.task.id))
    }

    /// A representative view for the event catalog's example payload — a plain, ready, childless
    /// task. Built from real fields (not a hand-written JSON string) so the published example is
    /// always a valid `TaskView`.
    #[must_use]
    pub(crate) fn example() -> Self {
        Self {
            task: Task {
                id: "t1".into(),
                title: "ship it".into(),
                details: None,
                status: TaskStatus::Open,
                project: None,
                cwd: None,
                parent: None,
                tag: None,
                assignee: None,
                created_at: 1_700_000_000,
                updated_at: 1_700_000_000,
                input: None,
                output_schema: None,
                result: None,
                workflow_run: None,
                step_key: None,
                give_up_reason: None,
            },
            effective: EffectiveStatus::Ready,
            children_total: 0,
            children_open: 0,
        }
    }
}

/// The `adi.tasks.deleted` payload: only the id of the removed task — the task itself is gone, so
/// there is no view to carry.
// A dedicated type (not an ad-hoc `json!`) so the emitted body and the published JSON Schema come
// from one definition and can never drift.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TaskDeleted {
    /// The id of the task that was deleted.
    pub id: String,
}

impl TaskDeleted {
    /// Build the event for the removed task's id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

/// The id counters plus the task list — the whole-table working set [`crate::db`] loads from
/// SQLite and saves back inside one transaction, and (via `Deserialize`) the shape a legacy
/// `tasks.json` is parsed into on a one-time migration. `next_id` numbers project-less tasks
/// (the legacy `t<n>` scheme); `seq` holds a per-project-key high-water mark so project-scoped
/// tasks get stable, never-reused Jira-style `<KEY>-<n>` ids (a deleted top task's number is not
/// handed out again).
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct TasksDoc {
    #[serde(default)]
    pub(crate) next_id: u64,
    #[serde(default)]
    pub(crate) seq: BTreeMap<String, u64>,
    #[serde(default)]
    pub(crate) tasks: Vec<Task>,
}

/// Everything a new task can be created with — the superset
/// [`Tasks::create`](crate::Tasks::create) is a six-field front door onto, for callers (a workflow
/// run's `ctx.task`, chiefly) that also need to set the workflow-step and structured-data fields.
/// `..NewTask::default()` for anything not given.
#[derive(Debug, Default)]
pub struct NewTask {
    /// Short one-line title.
    pub title: String,
    /// Optional longer details / notes — a task's instructions, when it has any.
    pub details: Option<String>,
    /// Optional associated adi project id.
    pub project: Option<String>,
    /// Optional free-form tag / label.
    pub tag: Option<String>,
    /// Optional parent task id.
    pub parent: Option<String>,
    /// Where this task's work happens; inherited from the parent when absent.
    pub cwd: Option<String>,
    /// Optional assignee, set at creation rather than through a later
    /// [`update`](crate::Tasks::update) — a workflow step is usually created already assigned.
    pub assignee: Option<String>,
    /// The input a run picking this up is given.
    pub input: Option<serde_json::Value>,
    /// The JSON Schema a result must satisfy to complete this task.
    pub output_schema: Option<serde_json::Value>,
    /// The workflow run this task is a step of. Requires `step_key` to also be set.
    pub workflow_run: Option<String>,
    /// This task's key within `workflow_run`. Requires `workflow_run` to also be set.
    pub step_key: Option<String>,
}

/// A partial update to a task; `None` fields are left unchanged. Status is deliberately absent —
/// it moves only through the dedicated [`complete`](crate::Tasks::complete) /
/// [`archive`](crate::Tasks::archive) / [`reopen`](crate::Tasks::reopen) methods.
#[derive(Debug, Default)]
pub struct TaskPatch {
    /// New title (unchanged if `None`).
    pub title: Option<String>,
    /// New details; `Some("")` clears, `None` leaves unchanged.
    pub details: Option<String>,
    /// New tag; `Some("")` clears, `None` leaves unchanged.
    pub tag: Option<String>,
    /// New assignee; `Some("")` clears, `None` leaves unchanged.
    pub assignee: Option<String>,
    /// New working directory; `Some("")` clears, `None` leaves unchanged.
    pub cwd: Option<String>,
    /// A requested parent change: `None` leaves it; `Some("")` detaches to root; `Some(id)` sets.
    pub parent: Option<String>,
}

/// A resolved, validated parent change to apply to a task.
#[derive(Debug)]
pub(crate) enum ParentChange {
    /// Leave the parent as-is.
    Keep,
    /// Detach to root (no parent).
    Clear,
    /// Set the parent to this (existing, non-cycling) id.
    Set(String),
}

// ---- derivation (pure, over the full task list) ----------------------------------------

/// The direct children of `id` in `tasks` (those whose `parent` equals `id`).
pub(crate) fn direct_children<'a>(
    tasks: &'a [Task],
    id: &'a str,
) -> impl Iterator<Item = &'a Task> {
    tasks
        .iter()
        .filter(move |t| t.parent.as_deref() == Some(id))
}

/// The computed status of `task` given the full task list: `archived`/`done` mirror the stored
/// status; an open task is `blocked` while any direct child is still open, else `ready`.
pub(crate) fn effective_status(task: &Task, tasks: &[Task]) -> EffectiveStatus {
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
pub(crate) fn descendants(tasks: &[Task], root: &str) -> Vec<String> {
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
pub(crate) fn would_cycle(tasks: &[Task], task_id: &str, new_parent: &str) -> bool {
    let mut cursor = Some(new_parent.to_string());
    while let Some(pid) = cursor {
        if pid == task_id {
            return true;
        }
        cursor = tasks
            .iter()
            .find(|t| t.id == pid)
            .and_then(|t| t.parent.clone());
    }
    false
}

/// The Jira-style key for a project id: its uppercase form (`my-app` → `MY-APP`). A project's
/// task ids are `<KEY>-<n>`, so the key is what groups them and gives the shared human-readable
/// prefix. Deriving it from the id keeps keys unique without a separate stored field.
pub(crate) fn project_key(project: &str) -> String {
    project.trim().to_ascii_uppercase()
}

/// The trailing number of a `<KEY>-<n>` id whose prefix is exactly `key`, or `None` when it isn't
/// one of that key's ids (the legacy `t<n>` ids and other keys' ids never match).
fn id_num(id: &str, key: &str) -> Option<u64> {
    id.strip_prefix(key)?.strip_prefix('-')?.parse().ok()
}

/// The highest task number already in use under `key`, or 0 if none — seeds the per-key counter
/// so ids stay unique even for tasks created before the counter existed (or under a case-variant
/// project id that maps to the same key).
pub(crate) fn max_num_for_key(tasks: &[Task], key: &str) -> u64 {
    tasks
        .iter()
        .filter_map(|t| id_num(&t.id, key))
        .max()
        .unwrap_or(0)
}
