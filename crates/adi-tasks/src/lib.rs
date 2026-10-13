//! adi-tasks — the adi task tree: a pure library (no CLI, no daemon) over the shared
//! [`adi_config`] store. State lives in one SQLite database (`tasks.db`) under the `tasks` module
//! dir of `~/.adi/mono`, replacing the old `tasks.json` — see the private `db` module for the
//! schema and `Tasks::migrate` for the one-time import of a pre-SQLite file.
//!
//! Tasks form a tree via each task's optional `parent`. Only three states are *stored*
//! (`open` / `done` / `archived`); a task's richer **effective** status (`ready` / `blocked` /
//! `done` / `archived`) is *computed* from that stored state plus its direct children, never
//! persisted — an open task is `blocked` while any direct child is still open, else `ready`.
//!
//! # Why one transaction around a whole-table load/mutate/save
//!
//! `tasks.json` used to be read whole, edited in memory, and written back whole with no lock at
//! all — two processes racing a mutation could each save over the other's edit. Every method here
//! instead opens one SQLite transaction (`BEGIN IMMEDIATE`, set as the connection's default),
//! loads the very same whole-table working set the JSON document used to be (`TasksDoc`), mutates
//! it with the exact same pure helpers in the private `task` module, and saves it back before
//! committing. SQLite's own writer lock makes that atomic across processes — a second writer
//! blocks until the first commits, rather than clobbering it — at the cost of a full
//! delete-then-reinsert on every save, which is no heavier than the whole-file rewrite it replaces
//! at the size a task tree actually reaches.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-tasks-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_tasks::Tasks;
//!
//! # let store = Tasks::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Tasks::open();
//! let root = store.create("ship it".into(), None, None, None, None, None)?;
//! assert_eq!(root.task.id, "t1");
//!
//! let child = store.create("subtask".into(), None, None, None, Some("t1".into()), None)?;
//! // The parent is now blocked by its still-open child.
//! assert_eq!(store.get("t1")?.children_open, 1);
//!
//! store.complete(&child.task.id, None)?;
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_tasks::Error>(())
//! ```

mod db;
mod error;
mod events;
mod task;

use std::path::PathBuf;
use std::rc::Rc;

use adi_config::{Config, Module, now_unix};
use rusqlite::Connection;

pub use error::{Error, Result};
pub use events::{
    TaskArchived, TaskCompleted, TaskCreated, TaskGaveUp, TaskReopened, TaskUpdated, event_types,
};
pub use task::{EffectiveStatus, NewTask, Task, TaskDeleted, TaskPatch, TaskStatus, TaskView};

use task::{
    ParentChange, TasksDoc, clean, descendants, max_num_for_key, project_key, would_cycle,
};

/// The config module (`~/.adi/mono/tasks`) the tracker persists under.
const MODULE: &str = "tasks";
/// Legacy module name used by the old task store location. Migrated once and copied forward so
/// existing task trees keep working.
const LEGACY_MODULE: &str = "mcp";
/// The tracker's pre-SQLite document — read once, as a migration source, then renamed to
/// [`BACKUP_FILE`] so it is never read (and never re-imported) again.
const TASKS_FILE: &str = "tasks.json";
/// Where a migrated `tasks.json` is renamed to, in the same module dir it was read from — the
/// backup the migration keeps, so it is reversible: delete `tasks.db`, rename this back, and the
/// store reads the pre-migration data again.
const BACKUP_FILE: &str = "tasks.json.bak";
/// The store's SQLite database.
const DB_FILE: &str = "tasks.db";

/// The task tree store: reads and writes `tasks.db` under the `tasks` module dir. Cheap to clone;
/// all state is on disk.
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

    /// The `tasks` module directory: `~/.adi/mono/tasks` (where `tasks.db` lives).
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.module().dir().to_path_buf()
    }

    /// The `tasks` config module handle.
    fn module(&self) -> Module {
        self.config.module(MODULE)
    }

    /// The pre-removal storage location (`~/.adi/mono/mcp`), read only as a migration source when
    /// the current module has no `tasks.json` of its own waiting to be imported.
    fn legacy_module(&self) -> Module {
        self.config.module(LEGACY_MODULE)
    }

    fn db_path(&self) -> PathBuf {
        self.module().raw_path(DB_FILE)
    }

    /// This thread's connection, opening (and preparing) it on first use.
    fn conn(&self) -> Result<Rc<Connection>> {
        self.module()
            .ensure_dir()
            .map_err(|e| Error::Store(e.into()))?;
        db::conn(&self.db_path())
    }

    /// Run `f` over the whole-table working set inside one write transaction: migrate a waiting
    /// `tasks.json` if there is one, load the current state, let `f` mutate it, save it back, and
    /// commit — all atomically, so no other writer's transaction can interleave. See the module
    /// docs for why this is one transaction rather than three.
    ///
    /// # Errors
    /// Whatever `f` returns, plus any transaction or store I/O failure.
    fn with_doc<T>(&self, f: impl FnOnce(&mut TasksDoc) -> Result<T>) -> Result<T> {
        let conn = self.conn()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| db::sql_err("begin a transaction in", e))?;
        self.migrate(&tx)?;
        let mut doc = db::load(&tx)?;
        let result = f(&mut doc)?;
        db::save(&tx, &doc)?;
        tx.commit()
            .map_err(|e| db::sql_err("commit a transaction in", e))?;
        Ok(result)
    }

    /// Run a read-only `f` over the current working set. Takes the same write-locked transaction
    /// [`with_doc`](Self::with_doc) does (rather than a plain read) so a store nobody has written
    /// to since the old `tasks.json` days still migrates on its very first `list`/`get` — but
    /// saves nothing back.
    ///
    /// # Errors
    /// Whatever `f` returns, plus any transaction or store I/O failure.
    fn with_doc_readonly<T>(&self, f: impl FnOnce(&TasksDoc) -> Result<T>) -> Result<T> {
        let conn = self.conn()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| db::sql_err("begin a transaction in", e))?;
        self.migrate(&tx)?;
        let doc = db::load(&tx)?;
        let result = f(&doc)?;
        tx.commit()
            .map_err(|e| db::sql_err("commit a transaction in", e))?;
        Ok(result)
    }

    /// Import a legacy `tasks.json` (the current module's own, or else the pre-removal `mcp`
    /// module's) the one time it still exists, then rename it to [`BACKUP_FILE`] so it is never
    /// read again. Runs inside `tx`'s write lock: two processes racing this both import the same
    /// bytes (harmless — whichever one's `tx` commits first wins the rename, and the other's
    /// `migrate` sees no file left to import and no-ops).
    ///
    /// # Errors
    /// Any I/O or parse failure other than the source file being absent.
    fn migrate(&self, tx: &Connection) -> Result<()> {
        let module = self.module();
        let legacy = self.legacy_module();
        let (source, backup, bytes) = if let Some(bytes) = module
            .read_raw(TASKS_FILE)
            .map_err(|e| Error::Store(e.into()))?
        {
            (
                module.raw_path(TASKS_FILE),
                module.raw_path(BACKUP_FILE),
                bytes,
            )
        } else if let Some(bytes) = legacy
            .read_raw(TASKS_FILE)
            .map_err(|e| Error::Store(e.into()))?
        {
            (
                legacy.raw_path(TASKS_FILE),
                legacy.raw_path(BACKUP_FILE),
                bytes,
            )
        } else {
            return Ok(());
        };
        let doc: TasksDoc = serde_json::from_slice(&bytes).map_err(|e| Error::Store(e.into()))?;
        db::save(tx, &doc)?;
        match std::fs::rename(&source, &backup) {
            Ok(()) => Ok(()),
            // Another process's `tx` already won this race and moved it away.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Store(e.into())),
        }
    }

    /// Publish a typed `adi.tasks.*` event (a task view, or a task id for a delete) onto
    /// the shared event bus. Best-effort and fire-and-forget: this store neither knows nor cares
    /// whether anything subscribes, and a spool failure must never fail the mutation that caused
    /// it. Emitted against **this store's** [`Config`], so a scratch store stays isolated.
    fn emit(&self, event: &impl adi_events::Event) {
        let _ = adi_events::Events::with_config(self.config.clone()).emit_event(event);
    }

    /// Create a new `open` task. If `parent` is given (and non-blank) it must already exist. A
    /// six-field front door onto [`create_full`](Self::create_full) for the common case.
    ///
    /// The new task's id follows its project: a project-scoped task gets a Jira-style `<KEY>-<n>`
    /// id (`KEY` is the uppercased project id, `n` a per-key counter); a project-less task keeps
    /// the legacy global `t<n>` id. When no project is given but a parent is, the task inherits
    /// the parent's project so a subtask shares the parent's key — and its `cwd` with it, so
    /// decomposing a task keeps every piece pointed at the same directory.
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
        cwd: Option<String>,
    ) -> Result<TaskView> {
        self.create_full(NewTask {
            title,
            details,
            project,
            tag,
            parent,
            cwd,
            ..NewTask::default()
        })
    }

    /// Create a new `open` task from the full [`NewTask`] shape — the door a workflow run's
    /// `ctx.task` creates a step through, with `input`/`output_schema`/`workflow_run`/`step_key`
    /// alongside everything [`create`](Self::create) takes.
    ///
    /// # Errors
    /// [`Error::ParentMissing`] for an unknown parent id, [`Error::InvalidOutputSchema`] if
    /// `output_schema` is not valid JSON Schema, [`Error::DuplicateStepId`] if `workflow_run` and
    /// `step_key` repeat a stepId already in use, or [`Error::Store`] on an I/O failure.
    pub fn create_full(&self, new: NewTask) -> Result<TaskView> {
        if let Some(schema) = &new.output_schema {
            validate_schema(schema)?;
        }
        let NewTask {
            title,
            details,
            project,
            tag,
            parent,
            cwd,
            assignee,
            input,
            output_schema,
            workflow_run,
            step_key,
        } = new;
        let view = self.with_doc(move |doc| {
            let parent = clean(parent);
            if let Some(pid) = parent.as_deref()
                && !doc.tasks.iter().any(|t| t.id == pid)
            {
                return Err(Error::ParentMissing(pid.to_string()));
            }
            // An explicit value wins; otherwise a subtask inherits from its parent — the project
            // so it lands under the same Jira key, and the working directory because decomposing
            // a task does not move the work: a subtask of "migrate <repo>" is still about that
            // repo, and re-typing the path on every subtask is how one of them ends up pointed
            // somewhere else.
            let inherited = parent
                .as_deref()
                .and_then(|pid| doc.tasks.iter().find(|t| t.id == pid));
            let mut project = clean(project);
            if project.is_none() {
                project = inherited.and_then(|t| t.project.clone());
            }
            let mut cwd = clean(cwd);
            if cwd.is_none() {
                cwd = inherited.and_then(|t| t.cwd.clone());
            }
            if let (Some(run), Some(key)) = (&workflow_run, &step_key)
                && doc.tasks.iter().any(|t| {
                    t.workflow_run.as_deref() == Some(run) && t.step_key.as_deref() == Some(key)
                })
            {
                return Err(Error::DuplicateStepId(format!("{run}/{key}")));
            }
            let id = match project.as_deref() {
                Some(p) => {
                    let key = project_key(p);
                    // Seed the counter from any existing ids so it never collides, then take the
                    // next.
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
                cwd,
                parent,
                tag: clean(tag),
                assignee: clean(assignee),
                created_at: now,
                updated_at: now,
                input,
                output_schema,
                result: None,
                workflow_run,
                step_key,
                give_up_reason: None,
            };
            doc.tasks.push(task);
            view_of(doc, &id)
        })?;
        self.emit(&TaskCreated::new(&view));
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
        let project = clean(project);
        let tag = clean(tag);
        self.with_doc_readonly(|doc| {
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
        })
    }

    /// Get one task view by id.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn get(&self, id: &str) -> Result<TaskView> {
        self.with_doc_readonly(|doc| view_of(doc, id))
    }

    /// Edit a task's fields (never its status). Reparenting is validated against the tree.
    ///
    /// # Errors
    /// [`Error::NotFound`] / [`Error::ParentMissing`] / [`Error::Cycle`] for a bad edit, or
    /// [`Error::Store`] on an I/O failure.
    pub fn update(&self, id: &str, patch: TaskPatch) -> Result<TaskView> {
        let view = self.with_doc(|doc| {
            let Some(idx) = doc.tasks.iter().position(|t| t.id == id) else {
                return Err(Error::NotFound(id.to_string()));
            };
            // Resolve any parent change against the current tree before taking a mutable borrow.
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
            if let Some(cwd) = patch.cwd {
                task.cwd = clean(Some(cwd));
            }
            match parent_change {
                ParentChange::Keep => {}
                ParentChange::Clear => task.parent = None,
                ParentChange::Set(pid) => task.parent = Some(pid),
            }
            task.updated_at = now_unix();
            view_of(doc, id)
        })?;
        self.emit(&TaskUpdated::new(&view));
        Ok(view)
    }

    /// Mark a task `done` (idempotent). An archived task must be reopened first. Open direct
    /// children do not block completion — callers surface [`TaskView::children_open`] as a
    /// warning.
    ///
    /// If the task has an [`output_schema`](Task::output_schema), `result` is required and
    /// validated against it before anything is stored — a result that fails validation refuses
    /// the whole call with the exact JSON pointer the mismatch is at
    /// ([`Error::ResultInvalid`]). A task with no `output_schema` accepts any `result` (or none)
    /// unchecked. Giving no `result` leaves whatever was already stored from an earlier attempt.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, [`Error::ReopenFirst`] if it is archived,
    /// [`Error::ResultRequired`] / [`Error::ResultInvalid`] when `output_schema` rejects the call,
    /// or [`Error::Store`] on an I/O failure.
    pub fn complete(&self, id: &str, result: Option<serde_json::Value>) -> Result<TaskView> {
        let view = self.with_doc(|doc| {
            let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
                return Err(Error::NotFound(id.to_string()));
            };
            if task.status == TaskStatus::Archived {
                return Err(Error::ReopenFirst);
            }
            if let Some(schema) = task.output_schema.clone() {
                let validator =
                    jsonschema::validator_for(&schema).map_err(|e| {
                        Error::InvalidOutputSchema(e.to_string())
                    })?;
                let Some(value) = result.as_ref() else {
                    return Err(Error::ResultRequired);
                };
                if let Err(e) = validator.validate(value) {
                    return Err(Error::ResultInvalid {
                        path: e.instance_path().to_string(),
                        message: e.to_string(),
                    });
                }
            }
            if let Some(r) = result {
                task.result = Some(r);
            }
            task.status = TaskStatus::Done;
            task.give_up_reason = None;
            task.updated_at = now_unix();
            view_of(doc, id)
        })?;
        self.emit(&TaskCompleted::new(&view));
        Ok(view)
    }

    /// Record that an agent gave up on this task, with its own reason. A stored *fact*
    /// ([`Task::give_up_reason`]), not a status change — `status` is left exactly as it was (open,
    /// almost always), so a dispatcher computes "stuck" from this rather than it doubling as one
    /// more stored lifecycle state. Cleared by [`complete`](Self::complete) and
    /// [`reopen`](Self::reopen).
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, [`Error::NotOpen`] if it is already `done` or
    /// `archived`, or [`Error::Store`] on an I/O failure.
    pub fn give_up(&self, id: &str, reason: &str) -> Result<TaskView> {
        let reason = reason.trim().to_string();
        let view = self.with_doc(|doc| {
            let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
                return Err(Error::NotFound(id.to_string()));
            };
            if task.status != TaskStatus::Open {
                return Err(Error::NotOpen(task.status.as_str().to_string()));
            }
            task.give_up_reason = Some(reason);
            task.updated_at = now_unix();
            view_of(doc, id)
        })?;
        self.emit(&TaskGaveUp::new(&view));
        Ok(view)
    }

    /// Archive a task. With `cascade`, also archive every still-open descendant (recursively);
    /// `done`/already-archived descendants are left untouched.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn archive(&self, id: &str, cascade: bool) -> Result<TaskView> {
        let view = self.with_doc(|doc| {
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
            view_of(doc, id)
        })?;
        self.emit(&TaskArchived::new(&view));
        Ok(view)
    }

    /// Reopen a `done` or `archived` task back to `open` (idempotent on an already-open task),
    /// clearing any [`give_up_reason`](Task::give_up_reason) — reopening is the decision that
    /// retires it.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn reopen(&self, id: &str) -> Result<TaskView> {
        let view = self.with_doc(|doc| {
            let Some(task) = doc.tasks.iter_mut().find(|t| t.id == id) else {
                return Err(Error::NotFound(id.to_string()));
            };
            let changed = task.status != TaskStatus::Open || task.give_up_reason.is_some();
            task.status = TaskStatus::Open;
            task.give_up_reason = None;
            if changed {
                task.updated_at = now_unix();
            }
            view_of(doc, id)
        })?;
        self.emit(&TaskReopened::new(&view));
        Ok(view)
    }

    /// Hard-delete a task, reparenting its direct children to the deleted task's parent so no
    /// dangling parent references remain.
    ///
    /// # Errors
    /// [`Error::NotFound`] if there is no such task, or [`Error::Store`] on an I/O failure.
    pub fn delete(&self, id: &str) -> Result<()> {
        self.with_doc(|doc| {
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
            Ok(())
        })?;
        self.emit(&TaskDeleted::new(id));
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

/// Check that `schema` is itself valid JSON Schema, without validating anything against it.
fn validate_schema(schema: &serde_json::Value) -> Result<()> {
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|e| Error::InvalidOutputSchema(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Tasks {
        db::forget_connections();
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
            .create(title.into(), None, None, None, parent.map(Into::into), None)
            .expect("create")
    }

    #[test]
    fn mutations_publish_events_onto_the_bus() {
        let store = scratch("events");
        let bus = adi_events::Events::with_config(store.config().clone());

        let created = mk(&store, "ship \"it\"\nnext", None);
        let updated = store
            .update(
                "t1",
                TaskPatch {
                    details: Some("ready to ship".into()),
                    ..TaskPatch::default()
                },
            )
            .expect("update");
        let completed = store.complete("t1", None).expect("complete");
        let archived = store.archive("t1", false).expect("archive");
        let reopened = store.reopen("t1").expect("reopen");
        store.delete("t1").expect("delete");

        let expected = [
            (
                "adi.tasks.created",
                serde_json::to_value(created).expect("created"),
            ),
            (
                "adi.tasks.updated",
                serde_json::to_value(updated).expect("updated"),
            ),
            (
                "adi.tasks.completed",
                serde_json::to_value(completed).expect("completed"),
            ),
            (
                "adi.tasks.archived",
                serde_json::to_value(archived).expect("archived"),
            ),
            (
                "adi.tasks.reopened",
                serde_json::to_value(reopened).expect("reopened"),
            ),
            (
                "adi.tasks.deleted",
                serde_json::to_value(TaskDeleted::new("t1")).expect("deleted"),
            ),
        ];
        let events = bus.drain().expect("drain");
        assert_eq!(
            events.len(),
            expected.len(),
            "each mutation emits one event"
        );
        for (event, (name, payload)) in events.into_iter().zip(expected) {
            assert_eq!(event.record.name, name);
            assert!(event.record.emitted_at > 0);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&event.record.payload).expect("payload"),
                payload,
                "{name} must retain its original flat payload"
            );
        }
    }

    #[test]
    fn task_mutations_succeed_when_the_event_spool_cannot_be_written() {
        let store = scratch("events-unwritable");
        let bus = adi_events::Events::with_config(store.config().clone());
        std::fs::create_dir_all(store.config().root()).expect("create root");
        // A regular file in the spool directory's place makes event writes fail on every OS.
        std::fs::write(bus.dir(), "not a directory").expect("block spool");

        mk(&store, "ship it", None);
        store.complete("t1", None).expect("complete without event spool");
        assert_eq!(
            store.get("t1").expect("persisted task").task.status,
            TaskStatus::Done
        );
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
            store.create("orphan".into(), None, None, None, Some("t99".into()), None),
            Err(Error::ParentMissing(_))
        ));
    }

    #[test]
    fn project_tasks_get_jira_ids_with_per_key_counters() {
        let store = scratch("jira");
        let a = store
            .create("a".into(), None, Some("demo".into()), None, None, None)
            .expect("create");
        let b = store
            .create("b".into(), None, Some("demo".into()), None, None, None)
            .expect("create");
        assert_eq!(a.task.id, "DEMO-1");
        assert_eq!(b.task.id, "DEMO-2");
        let c = store
            .create("c".into(), None, Some("my-app".into()), None, None, None)
            .expect("create");
        assert_eq!(c.task.id, "MY-APP-1");
        let d = store
            .create("d".into(), None, None, None, None, None)
            .expect("create");
        assert_eq!(d.task.id, "t1");
    }

    #[test]
    fn subtask_inherits_parent_project_and_key() {
        let store = scratch("inherit");
        let root = store
            .create("root".into(), None, Some("demo".into()), None, None, None)
            .expect("create");
        assert_eq!(root.task.id, "DEMO-1");
        let child = store
            .create(
                "child".into(),
                None,
                None,
                None,
                Some("DEMO-1".into()),
                None,
            )
            .expect("create");
        assert_eq!(child.task.id, "DEMO-2");
        assert_eq!(child.task.project.as_deref(), Some("demo"));
    }

    /// A task states where its work happens, and decomposing it keeps every piece pointed there —
    /// unless a subtask says otherwise, which is a deliberate move, not an omission.
    #[test]
    fn a_subtask_inherits_the_working_directory_unless_it_states_its_own() {
        let store = scratch("cwd-inherit");
        let root = store
            .create(
                "migrate".into(),
                None,
                None,
                None,
                None,
                Some("~/repos/thing".into()),
            )
            .expect("create");
        assert_eq!(root.task.cwd.as_deref(), Some("~/repos/thing"));

        let child = store
            .create(
                "step one".into(),
                None,
                None,
                None,
                Some(root.task.id.clone()),
                None,
            )
            .expect("create");
        assert_eq!(child.task.cwd.as_deref(), Some("~/repos/thing"));

        let elsewhere = store
            .create(
                "step two".into(),
                None,
                None,
                None,
                Some(root.task.id.clone()),
                Some("/tmp/scratch".into()),
            )
            .expect("create");
        assert_eq!(elsewhere.task.cwd.as_deref(), Some("/tmp/scratch"));
    }

    /// Editing it follows the same rule every optional field does: a value sets, a blank clears.
    #[test]
    fn editing_the_working_directory_sets_and_clears_it() {
        let store = scratch("cwd-edit");
        let task = store
            .create("fix".into(), None, None, None, None, None)
            .expect("create");
        assert_eq!(task.task.cwd, None);

        let patched = store
            .update(
                &task.task.id,
                TaskPatch {
                    cwd: Some("  /srv/app  ".into()),
                    ..TaskPatch::default()
                },
            )
            .expect("update");
        assert_eq!(patched.task.cwd.as_deref(), Some("/srv/app"));

        let cleared = store
            .update(
                &task.task.id,
                TaskPatch {
                    cwd: Some(String::new()),
                    ..TaskPatch::default()
                },
            )
            .expect("update");
        assert_eq!(cleared.task.cwd, None);
    }

    #[test]
    fn deleting_the_top_task_does_not_reuse_its_number() {
        let store = scratch("noreuse");
        store
            .create("a".into(), None, Some("demo".into()), None, None, None)
            .expect("create");
        store
            .create("b".into(), None, Some("demo".into()), None, None, None)
            .expect("create");
        store.delete("DEMO-2").expect("delete");
        let c = store
            .create("c".into(), None, Some("demo".into()), None, None, None)
            .expect("create");
        assert_eq!(c.task.id, "DEMO-3");
    }

    #[test]
    fn legacy_task_file_is_imported_and_the_file_is_kept_as_a_backup() {
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
                .module("mcp")
                .read_raw("tasks.json")
                .expect("read original legacy file")
                .is_none(),
            "the original is renamed away so it is never re-imported"
        );
        assert!(
            store
                .config()
                .module("mcp")
                .read_raw("tasks.json.bak")
                .expect("read backup")
                .is_some(),
            "the migration must keep the old file, just renamed"
        );

        // A second access, with the source file already gone, must not re-run the import (which
        // would otherwise fail on the now-duplicate id).
        assert_eq!(store.list(None, None, None, None).expect("list again").len(), 1);
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
        store.complete("t2", None).expect("complete child");
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
            store.complete("t1", None).expect("done").task.status,
            TaskStatus::Done
        );
        assert_eq!(
            store.complete("t1", None).expect("done again").task.status,
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
        assert!(matches!(store.complete("t1", None), Err(Error::ReopenFirst)));
        store.reopen("t1").expect("reopen");
        store.complete("t1", None).expect("complete after reopen");
    }

    #[test]
    fn archive_cascade_only_archives_open_descendants() {
        let store = scratch("cascade");
        mk(&store, "root", None);
        mk(&store, "open-child", Some("t1"));
        let done_child = mk(&store, "done-child", Some("t1"));
        store.complete(&done_child.task.id, None).expect("complete");
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
        store.complete("t2", None).expect("complete");
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

    #[test]
    fn create_full_sets_the_workflow_step_fields_and_enforces_unique_step_ids() {
        let store = scratch("steps");
        let step = store
            .create_full(NewTask {
                title: "research".into(),
                workflow_run: Some("run1".into()),
                step_key: Some("research".into()),
                input: Some(serde_json::json!({"topic": "adi"})),
                output_schema: Some(serde_json::json!({"type": "object"})),
                ..NewTask::default()
            })
            .expect("create step");
        assert_eq!(step.task.step_id(), Some("run1/research".to_string()));
        assert_eq!(step.task.input, Some(serde_json::json!({"topic": "adi"})));

        let duplicate = store.create_full(NewTask {
            title: "research again".into(),
            workflow_run: Some("run1".into()),
            step_key: Some("research".into()),
            ..NewTask::default()
        });
        assert!(matches!(duplicate, Err(Error::DuplicateStepId(ref s)) if s == "run1/research"));

        // A different run, or a different key, is a different stepId and is not rejected.
        assert!(
            store
                .create_full(NewTask {
                    title: "research, run 2".into(),
                    workflow_run: Some("run2".into()),
                    step_key: Some("research".into()),
                    ..NewTask::default()
                })
                .is_ok()
        );
    }

    #[test]
    fn create_full_rejects_an_invalid_output_schema() {
        let store = scratch("bad-schema");
        let err = store
            .create_full(NewTask {
                title: "step".into(),
                // `type` must be a string or array of strings, not a number — invalid JSON Schema.
                output_schema: Some(serde_json::json!({"type": 5})),
                ..NewTask::default()
            })
            .expect_err("invalid schema must be rejected at creation");
        assert!(matches!(err, Error::InvalidOutputSchema(_)));
    }

    #[test]
    fn complete_requires_and_validates_a_result_against_the_output_schema() {
        let store = scratch("result-validation");
        let step = store
            .create_full(NewTask {
                title: "step".into(),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {"ok": {"type": "boolean"}},
                    "required": ["ok"]
                })),
                ..NewTask::default()
            })
            .expect("create step");
        let id = step.task.id.clone();

        assert!(matches!(
            store.complete(&id, None),
            Err(Error::ResultRequired)
        ));

        let bad = store.complete(&id, Some(serde_json::json!({"ok": "not a bool"})));
        match bad {
            Err(Error::ResultInvalid { path, .. }) => assert_eq!(path, "/ok"),
            other => panic!("expected ResultInvalid, got {other:?}"),
        }

        let done = store
            .complete(&id, Some(serde_json::json!({"ok": true})))
            .expect("complete with a valid result");
        assert_eq!(done.task.status, TaskStatus::Done);
        assert_eq!(done.task.result, Some(serde_json::json!({"ok": true})));
    }

    #[test]
    fn complete_without_a_new_result_keeps_the_previous_one() {
        let store = scratch("keep-result");
        mk(&store, "task", None);
        store
            .complete("t1", Some(serde_json::json!({"a": 1})))
            .expect("complete with result");
        store.reopen("t1").expect("reopen");
        let done_again = store
            .complete("t1", None)
            .expect("complete again without a result");
        assert_eq!(done_again.task.result, Some(serde_json::json!({"a": 1})));
    }

    #[test]
    fn give_up_records_a_reason_without_changing_status_and_reopen_clears_it() {
        let store = scratch("give-up");
        mk(&store, "task", None);
        let gave_up = store
            .give_up("t1", "the API key is missing")
            .expect("give up");
        assert_eq!(gave_up.task.status, TaskStatus::Open);
        assert_eq!(
            gave_up.task.give_up_reason.as_deref(),
            Some("the API key is missing")
        );

        let reopened = store.reopen("t1").expect("reopen");
        assert_eq!(reopened.task.give_up_reason, None);
    }

    #[test]
    fn give_up_is_rejected_once_the_task_is_done_or_archived() {
        let store = scratch("give-up-finished");
        mk(&store, "task", None);
        store.complete("t1", None).expect("complete");
        assert!(matches!(
            store.give_up("t1", "too late"),
            Err(Error::NotOpen(_))
        ));

        mk(&store, "task2", None);
        store.archive("t2", false).expect("archive");
        assert!(matches!(
            store.give_up("t2", "too late"),
            Err(Error::NotOpen(_))
        ));
    }
}
