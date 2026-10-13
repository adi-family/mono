//! `workflows.db`, under its own `workflows-runs` module dir — the same one-file-per-domain
//! pattern `adi-schedules`' `schedules.db` and `adi-tasks`' `tasks.db` already follow.
//!
//! Three tables: `runs` (one row per started run — its `run_key`, which workflow and file
//! version it is, its input, why it started, and its root task once that exists), `step_journal`
//! (a resolved checkpoint's result, keyed `(run_key, step_key)` — what makes replay instant and
//! "rewind to step N" a truncation), and `step_pending` (the one piece of state a checkpoint that
//! has *not yet* resolved needs to remember across attempts — `ctx.waitFor`'s "the log position
//! when it starts", concretely; nothing else needs one, since a created task or child run row is
//! itself that memory for `ctx.task`/`ctx.run`/`ctx.ask`).

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, Row, params};

use adi_config::Config;

use crate::error::{Error, Result};

/// A module name distinct from `adi-schedules`' own `schedules` module: a schedule's due-slot
/// bookkeeping and a workflow's run bookkeeping are different domains that happen to both be
/// consulted when a schedule fires, not one store wearing two hats.
const MODULE: &str = "workflows-runs";
const DB_FILE: &str = "workflows.db";

/// The step journal's own cap on one resolved step's value — spec section E: "the step journal
/// (stepId → result, ≤256 KB, bigger data goes as a path in ctx.dir)". Measured on the JSON
/// text, which is what is actually stored.
pub const MAX_STEP_VALUE_BYTES: usize = 256 * 1024;

const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma synchronous = NORMAL;\n";

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS runs (
    id             INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    run_key        TEXT    NOT NULL UNIQUE,
    workflow       TEXT    NOT NULL,
    file_path      TEXT    NOT NULL,
    file_hash      TEXT    NOT NULL,
    input          TEXT    NOT NULL,
    cause          TEXT    NOT NULL,
    caused_by_run  TEXT,
    root_task      TEXT,
    finished_at    INTEGER,
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS runs_workflow ON runs (workflow);
CREATE INDEX IF NOT EXISTS runs_unfinished ON runs (finished_at);

CREATE TABLE IF NOT EXISTS step_journal (
    id         INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    run_key    TEXT    NOT NULL,
    step_key   TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    outcome    TEXT    NOT NULL,
    value      TEXT,
    task_id    TEXT,
    ref_key    TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(run_key, step_key)
);
CREATE INDEX IF NOT EXISTS step_journal_run ON step_journal (run_key, id);

CREATE TABLE IF NOT EXISTS step_pending (
    run_key  TEXT NOT NULL,
    step_key TEXT NOT NULL,
    state    TEXT NOT NULL,
    PRIMARY KEY (run_key, step_key)
);
";

/// `runs.caused_by_run` was added after `runs` first shipped — a dev store created before this
/// still has the table without the column, and `CREATE TABLE IF NOT EXISTS` above never alters an
/// existing table. Ignoring "duplicate column" is this crate's own small additive-migration idiom:
/// safe to run on every connect, since the two outcomes (the column already exists, or it gets
/// added) are both fine.
fn migrate(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "ALTER TABLE runs ADD COLUMN caused_by_run TEXT")?;
    add_column_if_missing(conn, "ALTER TABLE runs ADD COLUMN finished_at INTEGER")
}

/// Run an `ALTER TABLE ... ADD COLUMN` that is safe to repeat on every connect: the one failure
/// mode it's expected to hit (the column is already there, from an earlier connection or an
/// earlier version of this store) is swallowed; anything else is a real error.
fn add_column_if_missing(conn: &Connection, sql: &str) -> Result<()> {
    match conn.execute(sql, []) {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("duplicate column") => Ok(()),
        Err(e) => Err(Error::Store(anyhow::anyhow!("couldn't migrate runs: {e}"))),
    }
}

/// One started workflow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub run_key: String,
    pub workflow: String,
    pub file_path: String,
    /// The hash of the workflow file's content at the moment this run started — spec section E:
    /// "A run stays pinned to the file version it started with." Nothing re-reads the live file
    /// for an existing run; this is the fact that makes that possible once something does.
    pub file_hash: String,
    pub input: serde_json::Value,
    pub cause: serde_json::Value,
    /// The run whose own execution caused this one — the parent, for a `ctx.run` child; the
    /// event's own causer, propagated, for a run an event started that was itself an echo of
    /// some run's activity; `None` for a schedule, a manual start, or an event with no causer of
    /// its own. [`crate::causation`] walks this chain to refuse a workflow re-triggering itself,
    /// directly or through a few hops of other workflows.
    pub caused_by_run: Option<String>,
    pub root_task: Option<String>,
    /// When this run reached a terminal state (completed or failed), or `None` while still
    /// in flight — what [`WorkflowStore::list_unfinished`] filters on, so a tick's "attempt every
    /// unfinished run" never re-scans a run that is done.
    pub finished_at: Option<u64>,
    pub created_at: u64,
}

/// The workflow-run store: `workflows.db` under its own module dir.
#[derive(Debug, Clone)]
pub struct WorkflowStore {
    config: Config,
}

impl Default for WorkflowStore {
    fn default() -> Self {
        Self::open()
    }
}

impl WorkflowStore {
    /// Open the store backed by the standard config (`~/.adi/mono`, honoring `$ADI_DIR`).
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

    /// The `workflows-runs` module directory (where `workflows.db` lives).
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(MODULE).dir().to_path_buf()
    }

    fn connect(&self) -> Result<Connection> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't create {}: {e}", dir.display())))?;
        let conn = Connection::open(dir.join(DB_FILE))
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't open workflows.db: {e}")))?;
        conn.execute_batch(PRAGMAS)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't configure workflows.db: {e}")))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't create the workflows schema: {e}")))?;
        migrate(&conn)?;
        Ok(conn)
    }

    /// Claim `run_key` for a brand-new run — the one write every starter (schedule, event,
    /// manual, and a `ctx.run` child) funnels through, so "this run key exists" can never happen
    /// twice.
    ///
    /// # Errors
    /// [`Error::DuplicateRunKey`] if `run_key` is already in use, or [`Error::Store`] on a write
    /// failure.
    // One more positional field (`caused_by_run`) than WF-5's own run-starting half left this
    // at; a `NewRun` struct (mirroring `adi_tasks::NewTask`) would be the fix if another grows
    // onto it, but isn't worth the call-site churn for one field today.
    #[allow(clippy::too_many_arguments)]
    pub fn create_run(
        &self,
        run_key: &str,
        workflow: &str,
        file_path: &str,
        file_hash: &str,
        input: &serde_json::Value,
        cause: &serde_json::Value,
        caused_by_run: Option<&str>,
    ) -> Result<RunRow> {
        let conn = self.connect()?;
        let now = adi_config::now_unix();
        let outcome = conn.execute(
            "INSERT INTO runs (run_key, workflow, file_path, file_hash, input, cause, caused_by_run, root_task, finished_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8)",
            params![
                run_key,
                workflow,
                file_path,
                file_hash,
                input.to_string(),
                cause.to_string(),
                caused_by_run,
                i64::try_from(now).unwrap_or(i64::MAX),
            ],
        );
        match outcome {
            Ok(_) => Ok(RunRow {
                run_key: run_key.to_string(),
                workflow: workflow.to_string(),
                file_path: file_path.to_string(),
                file_hash: file_hash.to_string(),
                input: input.clone(),
                cause: cause.clone(),
                caused_by_run: caused_by_run.map(String::from),
                root_task: None,
                finished_at: None,
                created_at: now,
            }),
            Err(e) if is_unique_violation(&e) => Err(Error::DuplicateRunKey(run_key.to_string())),
            Err(e) => Err(Error::Store(anyhow::anyhow!("couldn't claim a run key: {e}"))),
        }
    }

    /// Record the root task a run's `pour` created — a second write because the task itself must
    /// name the run (`workflow_run = run_key`) before it can exist, so the two can't be one
    /// insert.
    ///
    /// # Errors
    /// [`Error::Store`] on a write failure.
    pub fn set_root_task(&self, run_key: &str, task_id: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE runs SET root_task = ?1 WHERE run_key = ?2",
            params![task_id, run_key],
        )
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't record the root task: {e}")))?;
        Ok(())
    }

    /// Mark `run_key` terminal (completed or failed) — what removes it from
    /// [`list_unfinished`](Self::list_unfinished), so a run the engine has already resolved is
    /// never re-attempted.
    ///
    /// # Errors
    /// [`Error::Store`] on a write failure.
    pub fn mark_finished(&self, run_key: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE runs SET finished_at = ?1 WHERE run_key = ?2",
            params![seq_i64(adi_config::now_unix()), run_key],
        )
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't mark a run finished: {e}")))?;
        Ok(())
    }

    /// Clear a run's terminal mark — [`crate::rewind`]'s own use: a rewound run needs
    /// [`Engine::run_tick`](crate::Engine::run_tick) to pick it back up and genuinely redo the
    /// discarded work, which a still-set `finished_at` would otherwise hide it from forever.
    ///
    /// # Errors
    /// [`Error::Store`] on a write failure.
    pub fn unfinish(&self, run_key: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE runs SET finished_at = NULL WHERE run_key = ?1",
            [run_key],
        )
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't unfinish a run: {e}")))?;
        Ok(())
    }

    /// Every run with a root task that hasn't [`mark_finished`](Self::mark_finished) yet, oldest
    /// first — what a tick drives: "attempt every unfinished run every tick", the same
    /// level-triggered sweep `adi_schedules::tick` and the dispatch tick already are.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn list_unfinished(&self) -> Result<Vec<RunRow>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT run_key, workflow, file_path, file_hash, input, cause, caused_by_run, \
                        root_task, finished_at, created_at \
                 FROM runs WHERE finished_at IS NULL AND root_task IS NOT NULL ORDER BY id",
            )
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read unfinished runs: {e}")))?;
        let rows = stmt
            .query_map([], row_to_run)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read unfinished runs: {e}")))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a run row: {e}")))
    }

    /// Hard-delete `run_key` and every journal/pending row of its own — used only by
    /// [`crate::rewind`], to free a child run's key for re-creation when the `ctx.run` step that
    /// made it is discarded. Does not touch anything the deleted run itself created (its root
    /// task, any grandchildren) — see the rewind module's own doc for why that's an accepted,
    /// documented limit rather than a recursive cascade.
    ///
    /// # Errors
    /// [`Error::Store`] on a write failure.
    pub fn delete_run(&self, run_key: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM runs WHERE run_key = ?1", [run_key])
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't delete a run: {e}")))?;
        conn.execute("DELETE FROM step_journal WHERE run_key = ?1", [run_key])
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't delete a run's journal: {e}")))?;
        conn.execute("DELETE FROM step_pending WHERE run_key = ?1", [run_key])
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't delete a run's pending state: {e}")))?;
        Ok(())
    }

    /// The run `run_key` names, or `None` if nothing has started it.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn get_run(&self, run_key: &str) -> Result<Option<RunRow>> {
        let conn = self.connect()?;
        conn.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM runs WHERE run_key = ?1"),
            [run_key],
            row_to_run,
        )
        .optional()
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a run: {e}")))
    }

    /// Every run started for `workflow`, oldest first — the audit trail a test asserts "started
    /// exactly once" against, same role as `adi_schedules::ScheduleStore::poured`.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn runs_for(&self, workflow: &str) -> Result<Vec<RunRow>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs WHERE workflow = ?1 ORDER BY created_at, run_key"
            ))
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read runs: {e}")))?;
        let rows = stmt
            .query_map([workflow], row_to_run)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read runs: {e}")))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a run row: {e}")))
    }

    // ---- step journal ---------------------------------------------------------------------

    /// Save a resolved checkpoint. Refuses a repeat of a key already journaled *within this
    /// run* — the `UNIQUE(run_key, step_key)` is what makes a genuine duplicate-key-in-one-
    /// attempt bug (one the TS `ctx` library's own in-process `Set` is the first line of
    /// defence against, since it can refuse before ever reaching the host) impossible to paper
    /// over by racing two attempts of the same run, which [`Engine::run_tick`](crate::Engine) by
    /// design never does concurrently.
    ///
    /// # Errors
    /// [`Error::JournalEntryTooLarge`] if the value's JSON text exceeds
    /// [`MAX_STEP_VALUE_BYTES`], [`Error::DuplicateStepKey`] if `step_key` is already journaled
    /// for this run, or [`Error::Store`] on a write failure.
    pub fn put_step(&self, run_key: &str, record: &StepRecord) -> Result<()> {
        let value = record.outcome.stored_value();
        if let Some(v) = &value
            && v.len() > MAX_STEP_VALUE_BYTES
        {
            return Err(Error::JournalEntryTooLarge(record.step_key.clone()));
        }
        let conn = self.connect()?;
        let outcome = conn.execute(
            "INSERT INTO step_journal (run_key, step_key, kind, outcome, value, task_id, ref_key, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run_key,
                record.step_key,
                record.kind.as_str(),
                record.outcome.as_str(),
                value,
                record.task_id,
                record.ref_key,
                seq_i64(adi_config::now_unix()),
            ],
        );
        match outcome {
            Ok(_) => {
                conn.execute(
                    "DELETE FROM step_pending WHERE run_key = ?1 AND step_key = ?2",
                    params![run_key, record.step_key],
                )
                .map_err(|e| Error::Store(anyhow::anyhow!("couldn't clear pending state: {e}")))?;
                Ok(())
            }
            Err(e) if is_unique_violation(&e) => {
                Err(Error::DuplicateStepKey(record.step_key.clone()))
            }
            Err(e) => Err(Error::Store(anyhow::anyhow!("couldn't save a step: {e}"))),
        }
    }

    /// The journaled resolution of `step_key` within `run_key`, if replay has already reached
    /// and resolved it.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn get_step(&self, run_key: &str, step_key: &str) -> Result<Option<StepRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT step_key, kind, outcome, value, task_id, ref_key \
             FROM step_journal WHERE run_key = ?1 AND step_key = ?2",
            params![run_key, step_key],
            row_to_step,
        )
        .optional()
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a step: {e}")))
    }

    /// Every journaled step of `run_key`, in the order they were first resolved — "step N" in
    /// "rewind to step N" means the Nth entry here.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn list_steps(&self, run_key: &str) -> Result<Vec<StepRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT step_key, kind, outcome, value, task_id, ref_key \
                 FROM step_journal WHERE run_key = ?1 ORDER BY id",
            )
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a run's steps: {e}")))?;
        let rows = stmt
            .query_map([run_key], row_to_step)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a run's steps: {e}")))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a step row: {e}")))
    }

    /// Discard every journal entry after the first `keep` (by resolution order), and every
    /// pending marker of the run — see [`crate::rewind`] for the whole operation, including what
    /// happens to a discarded step's own task or child run. Returns exactly the discarded
    /// records, oldest first, so the caller can act on each.
    ///
    /// # Errors
    /// [`Error::Store`] on a read or write failure.
    pub fn truncate_steps(&self, run_key: &str, keep: usize) -> Result<Vec<StepRecord>> {
        let all = self.list_steps(run_key)?;
        if keep >= all.len() {
            return Ok(Vec::new());
        }
        let discarded = all[keep..].to_vec();
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM step_journal WHERE run_key = ?1 AND id NOT IN \
             (SELECT id FROM step_journal WHERE run_key = ?1 ORDER BY id LIMIT ?2)",
            params![run_key, i64::try_from(keep).unwrap_or(i64::MAX)],
        )
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't truncate a run's steps: {e}")))?;
        conn.execute(
            "DELETE FROM step_pending WHERE run_key = ?1",
            [run_key],
        )
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't clear a run's pending state: {e}")))?;
        Ok(discarded)
    }

    // ---- pending state (checkpoints not yet resolved) --------------------------------------

    /// The pending marker `ctx.waitFor` saved the first time it reached this step, if any.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn get_pending(&self, run_key: &str, step_key: &str) -> Result<Option<serde_json::Value>> {
        let conn = self.connect()?;
        let found: Option<String> = conn
            .query_row(
                "SELECT state FROM step_pending WHERE run_key = ?1 AND step_key = ?2",
                params![run_key, step_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read pending state: {e}")))?;
        Ok(found.map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)))
    }

    /// Overwrite an already-journaled record's `task_id`/`ref_key` in place — the one case that
    /// legitimately rewrites a journal row rather than inserting a fresh one: a `ctx.run` step
    /// adding its child run key just after resolving, without `put_step` needing to carry that
    /// field through every other caller.
    ///
    /// # Errors
    /// [`Error::Store`] on a write failure.
    pub fn restamp_step(&self, run_key: &str, record: &StepRecord) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE step_journal SET task_id = ?1, ref_key = ?2 WHERE run_key = ?3 AND step_key = ?4",
            params![record.task_id, record.ref_key, run_key, record.step_key],
        )
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't restamp a step: {e}")))?;
        Ok(())
    }

    /// Save (or replace) the pending marker for a not-yet-resolved checkpoint.
    ///
    /// # Errors
    /// [`Error::Store`] on a write failure.
    pub fn put_pending(
        &self,
        run_key: &str,
        step_key: &str,
        state: &serde_json::Value,
    ) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO step_pending (run_key, step_key, state) VALUES (?1, ?2, ?3) \
             ON CONFLICT(run_key, step_key) DO UPDATE SET state = excluded.state",
            params![run_key, step_key, state.to_string()],
        )
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't save pending state: {e}")))?;
        Ok(())
    }
}

/// The column list every `runs` read shares, in the order [`row_to_run`] expects.
const RUN_COLUMNS: &str = "run_key, workflow, file_path, file_hash, input, cause, caused_by_run, \
                            root_task, finished_at, created_at";

fn row_to_run(row: &Row<'_>) -> rusqlite::Result<RunRow> {
    let input: String = row.get(4)?;
    let cause: String = row.get(5)?;
    let finished_at: Option<i64> = row.get(8)?;
    let created_at: i64 = row.get(9)?;
    Ok(RunRow {
        run_key: row.get(0)?,
        workflow: row.get(1)?,
        file_path: row.get(2)?,
        file_hash: row.get(3)?,
        input: serde_json::from_str(&input).unwrap_or(serde_json::Value::Null),
        cause: serde_json::from_str(&cause).unwrap_or(serde_json::Value::Null),
        caused_by_run: row.get(6)?,
        root_task: row.get(7)?,
        finished_at: finished_at.map(|v| u64::try_from(v).unwrap_or(0)),
        created_at: u64::try_from(created_at).unwrap_or(0),
    })
}

fn row_to_step(row: &Row<'_>) -> rusqlite::Result<StepRecord> {
    let kind: String = row.get(1)?;
    let outcome: String = row.get(2)?;
    let value: Option<String> = row.get(3)?;
    let value = value.map(|v| serde_json::from_str(&v).unwrap_or(serde_json::Value::Null));
    Ok(StepRecord {
        step_key: row.get(0)?,
        kind: StepKind::parse(&kind),
        outcome: StepOutcome::from_parts(&outcome, value),
        task_id: row.get(4)?,
        ref_key: row.get(5)?,
    })
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)
}

/// Bind a `u64` timestamp as SQLite's signed `INTEGER` — same narrowing [`crate::store`] applies
/// everywhere a counter crosses the FFI boundary.
fn seq_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Which `ctx` building block produced a [`StepRecord`] — kept mostly for [`crate::rewind`] to
/// know whether a discarded entry has a task (`Task`/`Ask`) or a child run (`Run`) to clean up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Step,
    Task,
    Run,
    Wait,
    Sleep,
    Ask,
}

impl StepKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Task => "task",
            Self::Run => "run",
            Self::Wait => "wait",
            Self::Sleep => "sleep",
            Self::Ask => "ask",
        }
    }

    /// Parse a stored kind, defaulting to [`Step`](Self::Step) for anything unrecognized (a
    /// forward-compat row written by a newer build) — matches [`adi_tasks::TaskStatus::parse`]'s
    /// own leniency convention for a column that isn't typed SQL.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "task" => Self::Task,
            "run" => Self::Run,
            "wait" => Self::Wait,
            "sleep" => Self::Sleep,
            "ask" => Self::Ask,
            _ => Self::Step,
        }
    }
}

/// How a checkpoint resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// A real value the workflow body receives — `ctx.step`'s return, a completed task's
    /// result, a matched event's payload, or `null` for `ctx.sleep`/a plain `ctx.ask` ack.
    Ok(serde_json::Value),
    /// `ctx.task`/`ctx.run`/`ctx.ask`'s underlying task was given up on — thrown into the
    /// workflow body as `TaskGaveUp`, never just returned.
    GaveUp(String),
    /// A `timeout` elapsed before the checkpoint resolved — returned to the workflow body as
    /// `{timed_out: true}`, never validated against whatever `output` shape the call declared
    /// (it isn't that shape; it's the documented escape hatch).
    TimedOut,
}

impl StepOutcome {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Ok(_) => "ok",
            Self::GaveUp(_) => "gave_up",
            Self::TimedOut => "timed_out",
        }
    }

    /// The JSON text actually stored in `step_journal.value` — `Ok`'s value, or `GaveUp`'s
    /// reason re-encoded as a plain JSON string (so one `value` column covers both); `TimedOut`
    /// stores nothing, since there is nothing but the tag to remember.
    fn stored_value(&self) -> Option<String> {
        match self {
            Self::Ok(v) => Some(serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())),
            Self::GaveUp(reason) => {
                Some(serde_json::to_string(reason).unwrap_or_else(|_| "null".to_string()))
            }
            Self::TimedOut => None,
        }
    }

    fn from_parts(outcome: &str, value: Option<serde_json::Value>) -> Self {
        match outcome {
            "gave_up" => Self::GaveUp(
                value
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
            ),
            "timed_out" => Self::TimedOut,
            _ => Self::Ok(value.unwrap_or(serde_json::Value::Null)),
        }
    }
}

/// One resolved checkpoint, as the journal stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRecord {
    pub step_key: String,
    pub kind: StepKind,
    pub outcome: StepOutcome,
    /// The task `ctx.task`/`ctx.ask` created, or a child run's root task for `ctx.run` — `None`
    /// for `ctx.step`/`ctx.waitFor`/`ctx.sleep`, which never create one.
    pub task_id: Option<String>,
    /// The child run key, for a `ctx.run` step only — what lets [`crate::rewind`] find the row
    /// to delete alongside its root task.
    pub ref_key: Option<String>,
}

impl StepRecord {
    #[must_use]
    pub fn new(step_key: impl Into<String>, kind: StepKind, outcome: StepOutcome) -> Self {
        Self {
            step_key: step_key.into(),
            kind,
            outcome,
            task_id: None,
            ref_key: None,
        }
    }

    #[must_use]
    pub fn with_task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    #[must_use]
    pub fn with_ref(mut self, ref_key: impl Into<String>) -> Self {
        self.ref_key = Some(ref_key.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(tag: &str) -> WorkflowStore {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        WorkflowStore::with_config(Config::with_root(root))
    }

    #[test]
    fn a_run_round_trips_and_is_found_by_key() {
        let store = scratch("roundtrip");
        assert_eq!(store.get_run("wf@2026-01-01T00:00:00Z").unwrap(), None);

        let run = store
            .create_run(
                "wf@2026-01-01T00:00:00Z",
                "wf",
                "/store/workflows/wf.ts",
                "deadbeef",
                &json!({}),
                &json!({"kind": "schedule"}),
                None,
            )
            .expect("create");
        assert_eq!(run.root_task, None);
        assert!(run.created_at > 0);

        let found = store.get_run("wf@2026-01-01T00:00:00Z").unwrap().unwrap();
        assert_eq!(found, run);
    }

    #[test]
    fn the_same_run_key_can_never_be_claimed_twice() {
        let store = scratch("claim-once");
        store
            .create_run("wf@slot", "wf", "/f.ts", "hash", &json!({}), &json!({}), None)
            .expect("first");
        let second = store.create_run("wf@slot", "wf", "/f.ts", "hash", &json!({}), &json!({}), None);
        assert!(matches!(second, Err(Error::DuplicateRunKey(ref k)) if k == "wf@slot"));
    }

    #[test]
    fn set_root_task_is_a_separate_write_that_shows_up_on_a_later_read() {
        let store = scratch("root-task");
        store
            .create_run("wf@slot", "wf", "/f.ts", "hash", &json!({}), &json!({}), None)
            .expect("create");
        store.set_root_task("wf@slot", "t1").expect("set root");
        let found = store.get_run("wf@slot").unwrap().unwrap();
        assert_eq!(found.root_task, Some("t1".to_string()));
    }

    #[test]
    fn runs_for_a_workflow_are_ordered_and_scoped_to_it() {
        let store = scratch("runs-for");
        store
            .create_run("a@1", "a", "/a.ts", "h", &json!({}), &json!({}), None)
            .expect("a1");
        store
            .create_run("b@1", "b", "/b.ts", "h", &json!({}), &json!({}), None)
            .expect("b1");
        store
            .create_run("a@2", "a", "/a.ts", "h", &json!({}), &json!({}), None)
            .expect("a2");

        let runs: Vec<String> = store
            .runs_for("a")
            .expect("runs_for")
            .into_iter()
            .map(|r| r.run_key)
            .collect();
        assert_eq!(runs, vec!["a@1".to_string(), "a@2".to_string()]);
    }

    #[test]
    fn input_and_cause_round_trip_as_real_json_not_strings() {
        let store = scratch("json");
        let input = json!({"topic": "quarterly review"});
        let cause = json!({"kind": "event", "seq": 42, "name": "adi.tasks.created"});
        store
            .create_run("wf@evt:42", "wf", "/f.ts", "h", &input, &cause, None)
            .expect("create");
        let found = store.get_run("wf@evt:42").unwrap().unwrap();
        assert_eq!(found.input, input);
        assert_eq!(found.cause, cause);
    }
}
