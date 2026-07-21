//! The task tree's data types — the stored [`Task`], the derived [`TaskView`], the two status
//! enums, a partial-update [`TaskPatch`] — plus the pure derivation helpers that compute a
//! task's effective status and validate tree edits. None of this touches disk; the [`Tasks`]
//! store in [`crate`] owns I/O.

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
                parent: None,
                tag: None,
                assignee: None,
                created_at: 1_700_000_000,
                updated_at: 1_700_000_000,
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

/// The on-disk document: the id counters plus the task list. `next_id` numbers project-less tasks
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

/// Trim a string, dropping it entirely when blank (so `""` clears an optional field).
pub(crate) fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
