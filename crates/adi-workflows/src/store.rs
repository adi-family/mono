//! `workflows.db`, under its own `workflows-runs` module dir — the same one-file-per-domain
//! pattern `adi-schedules`' `schedules.db` and `adi-tasks`' `tasks.db` already follow.
//!
//! Only the run-starting half of spec section E is built so far: one row per started run (its
//! `run_key`, which workflow and file version it is, its input and why it started, and its root
//! task once that exists). The step journal a running workflow's `ctx` checkpoints against —
//! and so rewind, replay, and duplicate-key refusal, which all act on *that* table — is further
//! WF-5 work; see the crate docs for exactly what's deferred.

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};

use adi_config::Config;

use crate::error::{Error, Result};

/// A module name distinct from `adi-schedules`' own `schedules` module: a schedule's due-slot
/// bookkeeping and a workflow's run bookkeeping are different domains that happen to both be
/// consulted when a schedule fires, not one store wearing two hats.
const MODULE: &str = "workflows-runs";
const DB_FILE: &str = "workflows.db";

const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma synchronous = NORMAL;\n";

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS runs (
    id          INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    run_key     TEXT    NOT NULL UNIQUE,
    workflow    TEXT    NOT NULL,
    file_path   TEXT    NOT NULL,
    file_hash   TEXT    NOT NULL,
    input       TEXT    NOT NULL,
    cause       TEXT    NOT NULL,
    root_task   TEXT,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS runs_workflow ON runs (workflow);
";

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
    pub root_task: Option<String>,
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
        Ok(conn)
    }

    /// Claim `run_key` for a brand-new run — the one write every starter (schedule, event, and
    /// eventually manual/child) funnels through, so "this run key exists" can never happen twice.
    ///
    /// # Errors
    /// [`Error::DuplicateRunKey`] if `run_key` is already in use, or [`Error::Store`] on a write
    /// failure.
    pub fn create_run(
        &self,
        run_key: &str,
        workflow: &str,
        file_path: &str,
        file_hash: &str,
        input: &serde_json::Value,
        cause: &serde_json::Value,
    ) -> Result<RunRow> {
        let conn = self.connect()?;
        let now = adi_config::now_unix();
        let outcome = conn.execute(
            "INSERT INTO runs (run_key, workflow, file_path, file_hash, input, cause, root_task, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
            params![
                run_key,
                workflow,
                file_path,
                file_hash,
                input.to_string(),
                cause.to_string(),
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
                root_task: None,
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

    /// The run `run_key` names, or `None` if nothing has started it.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn get_run(&self, run_key: &str) -> Result<Option<RunRow>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT run_key, workflow, file_path, file_hash, input, cause, root_task, created_at \
             FROM runs WHERE run_key = ?1",
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
            .prepare(
                "SELECT run_key, workflow, file_path, file_hash, input, cause, root_task, created_at \
                 FROM runs WHERE workflow = ?1 ORDER BY created_at, run_key",
            )
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read runs: {e}")))?;
        let rows = stmt
            .query_map([workflow], row_to_run)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read runs: {e}")))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a run row: {e}")))
    }
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    let input: String = row.get(4)?;
    let cause: String = row.get(5)?;
    let created_at: i64 = row.get(7)?;
    Ok(RunRow {
        run_key: row.get(0)?,
        workflow: row.get(1)?,
        file_path: row.get(2)?,
        file_hash: row.get(3)?,
        input: serde_json::from_str(&input).unwrap_or(serde_json::Value::Null),
        cause: serde_json::from_str(&cause).unwrap_or(serde_json::Value::Null),
        root_task: row.get(6)?,
        created_at: u64::try_from(created_at).unwrap_or(0),
    })
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)
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
            .create_run("wf@slot", "wf", "/f.ts", "hash", &json!({}), &json!({}))
            .expect("first");
        let second = store.create_run("wf@slot", "wf", "/f.ts", "hash", &json!({}), &json!({}));
        assert!(matches!(second, Err(Error::DuplicateRunKey(ref k)) if k == "wf@slot"));
    }

    #[test]
    fn set_root_task_is_a_separate_write_that_shows_up_on_a_later_read() {
        let store = scratch("root-task");
        store
            .create_run("wf@slot", "wf", "/f.ts", "hash", &json!({}), &json!({}))
            .expect("create");
        store.set_root_task("wf@slot", "t1").expect("set root");
        let found = store.get_run("wf@slot").unwrap().unwrap();
        assert_eq!(found.root_task, Some("t1".to_string()));
    }

    #[test]
    fn runs_for_a_workflow_are_ordered_and_scoped_to_it() {
        let store = scratch("runs-for");
        store
            .create_run("a@1", "a", "/a.ts", "h", &json!({}), &json!({}))
            .expect("a1");
        store
            .create_run("b@1", "b", "/b.ts", "h", &json!({}), &json!({}))
            .expect("b1");
        store
            .create_run("a@2", "a", "/a.ts", "h", &json!({}), &json!({}))
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
            .create_run("wf@evt:42", "wf", "/f.ts", "h", &input, &cause)
            .expect("create");
        let found = store.get_run("wf@evt:42").unwrap().unwrap();
        assert_eq!(found.input, input);
        assert_eq!(found.cause, cause);
    }
}
