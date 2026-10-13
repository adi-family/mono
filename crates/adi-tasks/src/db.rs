//! The SQLite store `tasks.db` lives in — connection pooling, schema, and the row <-> [`Task`]
//! mapping. Everything here is backend plumbing; the task-tree logic itself (effective status,
//! cycle checks, id assignment) is pure and lives in [`crate::task`], over the whole-table
//! [`TasksDoc`](crate::task::TasksDoc) this module loads and saves. See `lib.rs`'s module docs for
//! why a whole-table load/mutate/save inside one transaction is the right shape at this scale.
//!
//! One file, not the shared `db/global.db`: for the same reason `adi-agents`' `sessions.db` is its
//! own file (see that crate's `store/db.rs`) — platform bookkeeping has no business sitting where a
//! user's own `DROP TABLE` is a normal thing to type.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rusqlite::{Connection, Row};

use crate::error::Error;
use crate::task::{Task, TasksDoc};

/// Applied to every connection, in this order. `busy_timeout` must lead — switching journal mode
/// takes a lock of its own, and would otherwise fail outright against a store another process is
/// mid-write on. The adi-agents `sessions.db` docs work through this the same way.
const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma synchronous = NORMAL;\n";

/// The schema. `IF NOT EXISTS` throughout, so any process may be the one that creates it.
///
/// `tasks_step_id` is a *partial* unique index — `WHERE workflow_run IS NOT NULL AND step_key IS
/// NOT NULL` — so the thousands of ordinary tasks that are neither never collide with each other
/// over two columns that are simply absent; only two rows that both *do* name a run and a key can
/// violate it, which is exactly the `stepId = ${runId}/${key}` uniqueness the spec asks for.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS tasks (
    id             TEXT    NOT NULL PRIMARY KEY,
    title          TEXT    NOT NULL,
    details        TEXT,
    status         TEXT    NOT NULL DEFAULT 'open',
    project        TEXT,
    cwd            TEXT,
    parent         TEXT,
    tag            TEXT,
    assignee       TEXT,
    created_at     INTEGER NOT NULL DEFAULT 0,
    updated_at     INTEGER NOT NULL DEFAULT 0,
    input          TEXT,
    output_schema  TEXT,
    result         TEXT,
    workflow_run   TEXT,
    step_key       TEXT,
    give_up_reason TEXT
);
CREATE INDEX IF NOT EXISTS tasks_parent ON tasks (parent);
CREATE INDEX IF NOT EXISTS tasks_project ON tasks (project);
CREATE UNIQUE INDEX IF NOT EXISTS tasks_step_id
    ON tasks (workflow_run, step_key)
    WHERE workflow_run IS NOT NULL AND step_key IS NOT NULL;
CREATE TABLE IF NOT EXISTS task_next_id (
    id    INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    value INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO task_next_id (id, value) VALUES (1, 0);
CREATE TABLE IF NOT EXISTS task_seq (
    key   TEXT    NOT NULL PRIMARY KEY,
    value INTEGER NOT NULL
);
";

/// The columns of a task, in the order [`row_to_task`] reads them and [`insert_task`] writes them.
const TASK_COLUMNS: &str = "id, title, details, status, project, cwd, parent, tag, assignee, \
                             created_at, updated_at, input, output_schema, result, workflow_run, \
                             step_key, give_up_reason";

// One connection per thread per database — see adi-agents' `sessions.db` for the measurement that
// justifies caching rather than opening fresh each call.
thread_local! {
    static CONNS: RefCell<HashMap<PathBuf, Rc<Connection>>> = RefCell::new(HashMap::new());
}

/// The connection for `path` on this thread, opening (and preparing) it on first use.
pub(crate) fn conn(path: &Path) -> Result<Rc<Connection>, Error> {
    CONNS.with(|cache| {
        if let Some(existing) = cache.borrow().get(path) {
            return Ok(Rc::clone(existing));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| sql_err_io("create the directory for", e))?;
        }
        let mut connection = Connection::open(path).map_err(|e| sql_err("open", e))?;
        connection
            .execute_batch(PRAGMAS)
            .map_err(|e| sql_err("configure", e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| sql_err("create the schema in", e))?;
        // Every explicit transaction in the store reads before it writes. Reserve the writer
        // before taking that read snapshot: a deferred transaction cannot upgrade a stale WAL
        // snapshot, and SQLite returns BUSY immediately instead of honoring busy_timeout.
        connection.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
        let connection = Rc::new(connection);
        cache
            .borrow_mut()
            .insert(path.to_path_buf(), Rc::clone(&connection));
        Ok(connection)
    })
}

/// Drop this thread's cached connections. For tests, which build a store per case and would
/// otherwise keep a handle on a database they have just deleted.
#[cfg(test)]
pub(crate) fn forget_connections() {
    CONNS.with(|cache| cache.borrow_mut().clear());
}

pub(crate) fn sql_err(doing: &str, e: rusqlite::Error) -> Error {
    Error::Store(anyhow::anyhow!("couldn't {doing} the task database: {e}"))
}

fn sql_err_io(doing: &str, e: std::io::Error) -> Error {
    Error::Store(anyhow::anyhow!("couldn't {doing} the task database: {e}"))
}

/// Read every row — both counters and the whole task table — inside `tx`.
pub(crate) fn load(tx: &Connection) -> Result<TasksDoc, Error> {
    let next_id: u64 = tx
        .query_row(
            "SELECT value FROM task_next_id WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| sql_err("read the next-id counter in", e))?;
    let mut seq = BTreeMap::new();
    {
        let mut stmt = tx
            .prepare("SELECT key, value FROM task_seq")
            .map_err(|e| sql_err("read the per-project counters in", e))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| sql_err("read the per-project counters in", e))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| sql_err("read the per-project counters in", e))?
        {
            let key: String = row.get(0).map_err(|e| sql_err("read", e))?;
            let value: u64 = row.get(1).map_err(|e| sql_err("read", e))?;
            seq.insert(key, value);
        }
    }
    let mut tasks = Vec::new();
    {
        let mut stmt = tx
            .prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks ORDER BY created_at, id"
            ))
            .map_err(|e| sql_err("read the task table in", e))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| sql_err("read the task table in", e))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| sql_err("read the task table in", e))?
        {
            tasks.push(row_to_task(row).map_err(|e| sql_err("read a row of the task table in", e))?);
        }
    }
    Ok(TasksDoc {
        next_id,
        seq,
        tasks,
    })
}

/// Replace the whole stored state with `doc` inside `tx` — a full delete-then-reinsert, the same
/// cost shape as the `tasks.json` this replaced rewriting itself whole on every mutation, now
/// inside one transaction so a write that crosses it with another process's is serialized rather
/// than lost. Fine at the scale a task tree actually reaches; see `lib.rs`'s module docs.
pub(crate) fn save(tx: &Connection, doc: &TasksDoc) -> Result<(), Error> {
    tx.execute("DELETE FROM tasks", [])
        .map_err(|e| sql_err("clear the task table in", e))?;
    for task in &doc.tasks {
        insert_task(tx, task)?;
    }
    tx.execute(
        "UPDATE task_next_id SET value = ?1 WHERE id = 1",
        [doc.next_id],
    )
    .map_err(|e| sql_err("write the next-id counter in", e))?;
    tx.execute("DELETE FROM task_seq", [])
        .map_err(|e| sql_err("clear the per-project counters in", e))?;
    for (key, value) in &doc.seq {
        tx.execute(
            "INSERT INTO task_seq (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .map_err(|e| sql_err("write the per-project counters in", e))?;
    }
    Ok(())
}

fn insert_task(tx: &Connection, task: &Task) -> Result<(), Error> {
    tx.execute(
        &format!(
            "INSERT INTO tasks ({TASK_COLUMNS}) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)"
        ),
        rusqlite::params![
            task.id,
            task.title,
            task.details,
            task.status.as_str(),
            task.project,
            task.cwd,
            task.parent,
            task.tag,
            task.assignee,
            task.created_at,
            task.updated_at,
            task.input.as_ref().map(ToString::to_string),
            task.output_schema.as_ref().map(ToString::to_string),
            task.result.as_ref().map(ToString::to_string),
            task.workflow_run,
            task.step_key,
            task.give_up_reason,
        ],
    )
    .map_err(|e| match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::ConstraintViolation) => Error::DuplicateStepId(
            task.step_id()
                .unwrap_or_else(|| "<unknown>".to_string()),
        ),
        _ => sql_err("write a row to", e),
    })?;
    Ok(())
}

fn row_to_task(row: &Row<'_>) -> rusqlite::Result<Task> {
    let parse_json = |s: Option<String>| s.and_then(|s| serde_json::from_str(&s).ok());
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        details: row.get(2)?,
        status: crate::task::TaskStatus::parse(&row.get::<_, String>(3)?),
        project: row.get(4)?,
        cwd: row.get(5)?,
        parent: row.get(6)?,
        tag: row.get(7)?,
        assignee: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        input: parse_json(row.get(11)?),
        output_schema: parse_json(row.get(12)?),
        result: parse_json(row.get(13)?),
        workflow_run: row.get(14)?,
        step_key: row.get(15)?,
        give_up_reason: row.get(16)?,
    })
}
