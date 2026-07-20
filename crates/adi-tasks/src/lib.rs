//! adi-tasks — the adi task tree: a pure library (no CLI, no daemon) over the shared
//! [`adi_config`] store. State is one JSON document (`tasks.json`) under the `tasks` module dir of
//! `~/.adi/mono`, so tasks survive across processes; every method opens the store fresh and
//! writes atomically (via the configurator's temp-then-rename).
//!
//! Tasks form a tree via each task's optional `parent`. Only three states are *stored*
//! (`open` / `done` / `archived`); a task's richer **effective** status (`ready` / `blocked` /
//! `done` / `archived`) is *computed* from that stored state plus its direct children, never
//! persisted — an open task is `blocked` while any direct child is still open, else `ready`.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-tasks-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_tasks::Tasks;
//!
//! # let store = Tasks::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Tasks::open();
//! let root = store.create("ship it".into(), None, None, None, None)?;
//! assert_eq!(root.task.id, "t1");
//!
//! let child = store.create("subtask".into(), None, None, None, Some("t1".into()))?;
//! // The parent is now blocked by its still-open child.
//! assert_eq!(store.get("t1")?.children_open, 1);
//!
//! store.complete(&child.task.id)?;
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_tasks::Error>(())
//! ```

mod error;
mod task;

use std::path::PathBuf;

use adi_config::{Config, Module, now_unix};

pub use error::{Error, Result};
pub use task::{EffectiveStatus, Task, TaskPatch, TaskStatus, TaskView};

use task::{ParentChange, TasksDoc, clean, descendants, max_num_for_key, project_key, would_cycle};

/// The config module (`~/.adi/mono/tasks`) the tracker persists under.
const MODULE: &str = "tasks";
/// Legacy module name used by the old task store location. Loaded once and copied forward so
/// existing task trees keep working.
const LEGACY_MODULE: &str = "mcp";
/// The tracker's on-disk document.
const TASKS_FILE: &str = "tasks.json";

/// The task tree store: reads and writes the `tasks.json` document under the `tasks` module dir.
/// Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct Tasks {
    config: Config,
}

impl Default for Tasks {
    fn default() -> Self {
        Self::open()
    }
}

impl Tasks {
    /// Open the store backed by the standard config store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the store backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The store this tracker reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The `tasks` module directory: `~/.adi/mono/tasks` (where `tasks.json` lives).
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.module().dir().to_path_buf()
    }

    /// The `tasks` config module handle.
    fn module(&self) -> Module {
        self.config.module(MODULE)
    }

    /// The pre-removal storage location (`~/.adi/mono/mcp/tasks.json`), read only as a migration
    /// source when the current task file does not exist yet.
    fn legacy_module(&self) -> Module {
        self.config.module(LEGACY_MODULE)
    }

    fn load(&self) -> anyhow::Result<TasksDoc> {
        match self.module().read_raw(TASKS_FILE)? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)?),
            None => match self.legacy_module().read_raw(TASKS_FILE)? {
                Some(bytes) => {
                    let doc = serde_json::from_slice(&bytes)?;
                    self.save(&doc)?;
                    Ok(doc)
                }
                None => Ok(TasksDoc::default()),
            },
        }
    }

    fn save(&self, doc: &TasksDoc) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(doc)?;
        self.module().write_raw(TASKS_FILE, &bytes)?;
        Ok(())
    }

    /// Publish an `adi.tasks.*` event with `payload` (a task view, or `{id}` for a delete) onto
    /// the shared event bus. Best-effort and fire-and-forget: this store neither knows nor cares
    /// whether anything subscribes, and a spool failure must never fail the mutation that caused
    /// it. Emitted against **this store's** [`Config`], so a scratch store stays isolated.
    fn emit(&self, event: &str, payload: &impl serde::Serialize) {
        if let Ok(json) = serde_json::to_string(payload) {
            let _ = adi_events::Events::with_config(self.config.clone()).emit(event, json);
        }
    }

    /// Create a new `open` task. If `parent` is given (and non-blank) it must already exist.
    ///
    /// The new task's id follows its project: a project-scoped task gets a Jira-style `<KEY>-<n>`
    /// id (`KEY` is the uppercased project id, `n` a per-key counter); a project-less task keeps
    /// the legacy global `t<n>` id. When no project is given but a parent is, the task inherits
    /// the parent's project so a subtask shares the parent's key.
    ///
    /// # Errors
    /// [`Error::ParentMissing`] for an unknown parent id, or [`Error::Store`] on an I/O failure.
    pub fn create(
        &self,
        title: String,
        details: Option<String>,
        project: Option<String>,
        tag: Option<String>,
        parent: Option<String>,
    ) -> Result<TaskView> {
        let mut doc = self.load()?;
        let parent = clean(parent);
        if let Some(pid) = parent.as_deref()
            && !doc.tasks.iter().any(|t| t.id == pid)
        {
            return Err(Error::ParentMissing(pid.to_string()));
        }
        // An explicit project wins; otherwise a subtask inherits its parent's project so it
        // lands under the same Jira key.
        let mut project = clean(project);
        if project.is_none()
            && let Some(pid) = parent.as_deref()
        {
            project = doc
                .tasks
                .iter()
                .find(|t| t.id == pid)
                .and_then(|t| t.project.clone());
        }
        let id = match project.as_deref() {
            Some(p) => {
                let key = project_key(p);
                // Seed the counter from any existing ids so it never collides, then take the next.
                let seed = max_num_for_key(&doc.tasks, &key);
                let n = doc.seq.entry(key.clone()).or_insert(seed);
                *n += 1;
                format!("{key}-{n}")
            }
            None => {
                doc.next_id += 1;
                format!("t{}", doc.next_id)
            }
        };
        let now = now_unix();
        let task = Task {
            id: id.clone(),
            title,
            details: clean(details),
            status: TaskStatus::Open,
            project,
            parent,
            tag: clean(tag),
            assignee: None,
            created_at: now,
            updated_at: now,
        };
        doc.tasks.push(task);
        self.save(&doc)?;
        let view = view_of(&doc, &id)?;
        self.emit("adi.tasks.created", &view);
        Ok(view)
    }

    /// List task views, filtered by any provided stored/computed field.
    ///
    /// # Errors
    /// [`Error::Store`] on an I/O or deserialization failure.
    pub fn list(
        &self,
        project: Option<String>,
        tag: Option<String>,
        status: Option<TaskStatus>,
        effective: Option<EffectiveStatus>,
    ) -> Result<Vec<TaskView>> {
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
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn get(&self, id: &str) -> Result<TaskView> {
        view_of(&self.load()?, id)
    }

    /// Edit a task's fields (never its status). Reparenting is validated against the tree.
    ///
    /// # Errors
    /// [`Error::NotFound`] / [`Error::ParentMissing`] / [`Error::Cycle`] for a bad edit, or
    /// [`Error::Store`] on an I/O failure.
    pub fn update(&self, id: &str, patch: TaskPatch) -> Result<TaskView> {
        let mut doc = self.load()?;
        let Some(idx) = doc.tasks.iter().position(|t| t.id == id) else {
            return Err(Error::NotFound(id.to_string()));
        };
        // Resolve any parent change against the current tree before we take a mutable borrow.
        let parent_change = match patch.parent {
            None => ParentChange::Keep,
            Some(raw) => match clean(Some(raw)) {
                None => ParentChange::Clear,
                Some(pid) => {
                    if !doc.tasks.iter().any(|t| t.id == pid) {
                        return Err(Error::ParentMissing(pid));
                    }
                    if would_cycle(&doc.tasks, id, &pid) {
                        return Err(Error::Cycle);
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
        let view = view_of(&doc, id)?;
        self.emit("adi.tasks.updated", &view);
        Ok(view)
    }

    /// Mark a task `done` (idempotent). An archived task must be reopened first. Open direct
    /// children do not block completion — callers surface [`TaskView::children_open`] as a warning.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, [`Error::ReopenFirst`] if it is archived, or
    /// [`Error::Store`] on an I/O failure.
    pub fn complete(&self, id: &str) -> Result<TaskView> {
        let mut doc = self.load()?;
        let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
            return Err(Error::NotFound(id.to_string()));
        };
        if task.status == TaskStatus::Archived {
            return Err(Error::ReopenFirst);
        }
        task.status = TaskStatus::Done;
        task.updated_at = now_unix();
        self.save(&doc)?;
        let view = view_of(&doc, id)?;
        self.emit("adi.tasks.completed", &view);
        Ok(view)
    }

    /// Archive a task. With `cascade`, also archive every still-open descendant (recursively);
    /// `done`/already-archived descendants are left untouched.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn archive(&self, id: &str, cascade: bool) -> Result<TaskView> {
        let mut doc = self.load()?;
        if !doc.tasks.iter().any(|t| t.id == id) {
            return Err(Error::NotFound(id.to_string()));
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
        let view = view_of(&doc, id)?;
        self.emit("adi.tasks.archived", &view);
        Ok(view)
    }

    /// Reopen a `done` or `archived` task back to `open` (idempotent on an already-open task).
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn reopen(&self, id: &str) -> Result<TaskView> {
        let mut doc = self.load()?;
        let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
            return Err(Error::NotFound(id.to_string()));
        };
        if task.status != TaskStatus::Open {
            task.status = TaskStatus::Open;
            task.updated_at = now_unix();
        }
        self.save(&doc)?;
        let view = view_of(&doc, id)?;
        self.emit("adi.tasks.reopened", &view);
        Ok(view)
    }

    /// Hard-delete a task, reparenting its direct children to the deleted task's parent so no
    /// dangling parent references remain.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn delete(&self, id: &str) -> Result<()> {
        let mut doc = self.load()?;
        let Some(idx) = doc.tasks.iter().position(|t| t.id == id) else {
            return Err(Error::NotFound(id.to_string()));
        };
        let orphan_parent = doc.tasks[idx].parent.clone();
        for t in &mut doc.tasks {
            if t.parent.as_deref() == Some(id) {
                t.parent.clone_from(&orphan_parent);
            }
        }
        doc.tasks.remove(idx);
        self.save(&doc)?;
        self.emit("adi.tasks.deleted", &serde_json::json!({ "id": id }));
        Ok(())
    }
}

/// Build the [`TaskView`] of `id` from `doc`, or [`Error::NotFound`] if it is gone.
fn view_of(doc: &TasksDoc, id: &str) -> Result<TaskView> {
    match doc.tasks.iter().find(|t| t.id == id) {
        Some(task) => Ok(TaskView::of(task.clone(), &doc.tasks)),
        None => Err(Error::NotFound(id.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Tasks {
        let root = std::env::temp_dir().join(format!(
            "adi-tasks-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Tasks::with_config(Config::with_root(root))
    }

    fn mk(store: &Tasks, title: &str, parent: Option<&str>) -> TaskView {
        store
            .create(title.into(), None, None, None, parent.map(Into::into))
            .expect("create")
    }

    #[test]
    fn mutations_publish_events_onto_the_bus() {
        let store = scratch("events");
        let bus = adi_events::Events::with_config(store.config().clone());

        mk(&store, "ship it", None);
        store.complete("t1").expect("complete");

        let events: Vec<(String, String)> = bus
            .drain()
            .expect("drain")
            .into_iter()
            .map(|s| (s.record.name, s.record.payload))
            .collect();
        assert_eq!(events.len(), 2, "create + complete each emit one event");
        assert_eq!(events[0].0, "adi.tasks.created");
        assert_eq!(events[1].0, "adi.tasks.completed");
        // The payload is the task view — parseable JSON carrying the id (the Task fields are
        // flattened to the top level).
        let payload: serde_json::Value =
            serde_json::from_str(&events[0].1).expect("payload is JSON");
        assert_eq!(payload["id"], "t1");
    }

    #[test]
    fn create_assigns_incrementing_ids_and_defaults_to_open() {
        let store = scratch("ids");
        let a = mk(&store, "first", None);
        let b = mk(&store, "second", None);
        assert_eq!(a.task.id, "t1");
        assert_eq!(b.task.id, "t2");
        assert_eq!(a.task.status, TaskStatus::Open);
        assert_eq!(a.effective, EffectiveStatus::Ready);
    }

    #[test]
    fn create_with_parent_links_and_rejects_missing_parent() {
        let store = scratch("parent");
        mk(&store, "root", None);
        let child = mk(&store, "child", Some("t1"));
        assert_eq!(child.task.parent.as_deref(), Some("t1"));
        assert!(matches!(
            store.create("orphan".into(), None, None, None, Some("t99".into())),
            Err(Error::ParentMissing(_))
        ));
    }

    #[test]
    fn project_tasks_get_jira_ids_with_per_key_counters() {
        let store = scratch("jira");
        let a = store
            .create("a".into(), None, Some("demo".into()), None, None)
            .expect("create");
        let b = store
            .create("b".into(), None, Some("demo".into()), None, None)
            .expect("create");
        assert_eq!(a.task.id, "DEMO-1");
        assert_eq!(b.task.id, "DEMO-2");
        let c = store
            .create("c".into(), None, Some("my-app".into()), None, None)
            .expect("create");
        assert_eq!(c.task.id, "MY-APP-1");
        let d = store
            .create("d".into(), None, None, None, None)
            .expect("create");
        assert_eq!(d.task.id, "t1");
    }

    #[test]
    fn subtask_inherits_parent_project_and_key() {
        let store = scratch("inherit");
        let root = store
            .create("root".into(), None, Some("demo".into()), None, None)
            .expect("create");
        assert_eq!(root.task.id, "DEMO-1");
        let child = store
            .create("child".into(), None, None, None, Some("DEMO-1".into()))
            .expect("create");
        assert_eq!(child.task.id, "DEMO-2");
        assert_eq!(child.task.project.as_deref(), Some("demo"));
    }

    #[test]
    fn deleting_the_top_task_does_not_reuse_its_number() {
        let store = scratch("noreuse");
        store
            .create("a".into(), None, Some("demo".into()), None, None)
            .expect("create");
        store
            .create("b".into(), None, Some("demo".into()), None, None)
            .expect("create");
        store.delete("DEMO-2").expect("delete");
        let c = store
            .create("c".into(), None, Some("demo".into()), None, None)
            .expect("create");
        assert_eq!(c.task.id, "DEMO-3");
    }

    #[test]
    fn legacy_task_file_is_copied_to_the_tasks_module() {
        let store = scratch("legacy");
        store
            .config()
            .module("mcp")
            .write_raw(
                "tasks.json",
                br#"{
                  "next_id": 1,
                  "tasks": [{
                    "id": "t1",
                    "title": "legacy task",
                    "status": "open",
                    "created_at": 1,
                    "updated_at": 1
                  }]
                }"#,
            )
            .expect("write legacy task file");

        let tasks = store
            .list(None, None, None, None)
            .expect("load from legacy task file");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task.title, "legacy task");
        assert!(
            store
                .config()
                .module("tasks")
                .read_raw("tasks.json")
                .expect("read migrated task file")
                .is_some(),
            "loading the legacy file should materialize the new task file"
        );
    }

    #[test]
    fn effective_status_derives_from_direct_children() {
        let store = scratch("effective");
        mk(&store, "root", None);
        mk(&store, "child", Some("t1"));
        assert_eq!(
            store.get("t1").expect("get").effective,
            EffectiveStatus::Blocked
        );
        store.complete("t2").expect("complete child");
        assert_eq!(
            store.get("t1").expect("get").effective,
            EffectiveStatus::Ready
        );
    }

    #[test]
    fn complete_is_idempotent_and_reopen_restores_open() {
        let store = scratch("complete");
        mk(&store, "task", None);
        assert_eq!(
            store.complete("t1").expect("done").task.status,
            TaskStatus::Done
        );
        assert_eq!(
            store.complete("t1").expect("done again").task.status,
            TaskStatus::Done
        );
        assert_eq!(
            store.reopen("t1").expect("reopen").task.status,
            TaskStatus::Open
        );
    }

    #[test]
    fn archived_task_must_be_reopened_before_completing() {
        let store = scratch("archived");
        mk(&store, "task", None);
        store.archive("t1", true).expect("archive");
        assert!(matches!(store.complete("t1"), Err(Error::ReopenFirst)));
        store.reopen("t1").expect("reopen");
        store.complete("t1").expect("complete after reopen");
    }

    #[test]
    fn archive_cascade_only_archives_open_descendants() {
        let store = scratch("cascade");
        mk(&store, "root", None);
        mk(&store, "open-child", Some("t1"));
        let done_child = mk(&store, "done-child", Some("t1"));
        store.complete(&done_child.task.id).expect("complete");
        store.archive("t1", true).expect("archive cascade");
        assert_eq!(
            store.get("t2").expect("get").task.status,
            TaskStatus::Archived
        );
        assert_eq!(store.get("t3").expect("get").task.status, TaskStatus::Done);
    }

    #[test]
    fn reparenting_into_a_cycle_is_rejected() {
        let store = scratch("cycle");
        mk(&store, "root", None);
        mk(&store, "child", Some("t1"));
        let patch = TaskPatch {
            parent: Some("t2".into()),
            ..TaskPatch::default()
        };
        assert!(matches!(store.update("t1", patch), Err(Error::Cycle)));
    }

    #[test]
    fn delete_reparents_children_to_the_removed_tasks_parent() {
        let store = scratch("delete");
        mk(&store, "root", None);
        mk(&store, "mid", Some("t1"));
        mk(&store, "leaf", Some("t2"));
        store.delete("t2").expect("delete mid");
        assert_eq!(
            store.get("t3").expect("get").task.parent.as_deref(),
            Some("t1")
        );
        assert!(matches!(store.get("t2"), Err(Error::NotFound(_))));
    }

    #[test]
    fn list_filters_by_stored_and_effective_status() {
        let store = scratch("list");
        mk(&store, "a", None);
        mk(&store, "b", None);
        store.complete("t2").expect("complete");
        let open = store
            .list(None, None, Some(TaskStatus::Open), None)
            .expect("list open");
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].task.id, "t1");
        let done = store
            .list(None, None, None, Some(EffectiveStatus::Done))
            .expect("list done");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].task.id, "t2");
    }
}
