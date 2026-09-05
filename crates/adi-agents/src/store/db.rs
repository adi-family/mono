//! The database the session store keeps everything in, and the one-time import that fills it.
//!
//! # Why a database, and why this one
//!
//! Listing sessions used to be a directory walk: one `read_dir` per agent, then a read of every
//! session's sidecar and a `stat` of every transcript to find when it last spoke. A profile of the
//! live control panel under load put 5255 samples in `stat` and 1644 in `open` against 112 in JSON
//! parsing — the listing was not computing anything, it was asking the filesystem for metadata,
//! several thousand times a second. That is a shape that gets worse with every session and is
//! catastrophic on a spinning disk.
//!
//! This is a **private** database, `<sessions_dir>/sessions.db`, and deliberately not the shared
//! `db/global.db` that `adi-db` hands to tools and shows on the Database page: those tables belong
//! to whoever is using the platform, and platform bookkeeping has no business sitting where a
//! `DROP TABLE` from a user's own SQL is a normal thing to type.
//!
//! # What is *not* in here
//!
//! A session's **log** is still a file, and has to be: a spawned child needs a real file descriptor
//! to redirect stdout and stderr into, which is not something a row can be. So a session is a row
//! plus `<id>.log`, and everything a runner spools beside that log stays where the runner put it.
//!
//! # Cross-process, which is the whole reason the store owns no state
//!
//! The CLI, the app, and every trigger's child open this independently. WAL is what makes that
//! safe — readers never block the writer and the writer never blocks readers — and `busy_timeout`
//! turns the remaining writer-vs-writer contention into a short wait instead of an error. The
//! pragma order matters: `busy_timeout` must be set first, because switching journal mode takes a
//! lock of its own and would otherwise fail outright against a store another process is mid-write
//! on. (`adi-db` documents the same rule for the shared store; it is the same trap.)

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Applied to every connection, in this order. See the module docs for why `busy_timeout` leads.
const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma synchronous = NORMAL;\n\
    pragma foreign_keys = ON;\n";

/// The schema. `IF NOT EXISTS` throughout, so any process may be the one that creates it.
///
/// `turns`, `queue`, `questions` and `goals` all cascade off `sessions`: deleting a session takes
/// its messages, anything it was waiting to be told, and what it was for, in one statement rather
/// than five that could half-fail. The index on `sessions` is the listing's own order — newest
/// first, id as tiebreak — so the query that replaced the directory walk reads it straight off the
/// index without a sort, and `questions_open` / `goals_open` are partial for the same reason in
/// reverse: the inbox, the deadline sweep and the goal sweep ask only about the handful of rows
/// still outstanding among every one ever written.
///
/// `goals_by_id` is what lets a goal be found by its id alone rather than by the full key — the id
/// is quoted back to a run in every nudge, and typed by a shell that may not know which
/// conversation it is standing in. Unique because that lookup has to be unambiguous.
///
/// `attachments` is the one table that cascades off nothing, and cannot: an image is uploaded
/// before the message that carries it exists, so a newly stored row has no session to point at. It
/// is cleaned up by [`attachments`](super::attachments) instead — claimed when a turn records it,
/// swept when nobody ever did.
pub(super) const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS sessions (
    agent         TEXT    NOT NULL,
    id            TEXT    NOT NULL,
    backend       TEXT    NOT NULL DEFAULT '',
    cwd           TEXT    NOT NULL DEFAULT '',
    message       TEXT    NOT NULL DEFAULT '',
    started_at    INTEGER NOT NULL DEFAULT 0,
    last_activity INTEGER NOT NULL DEFAULT 0,
    hidden        INTEGER NOT NULL DEFAULT 0,
    starred       INTEGER NOT NULL DEFAULT 0,
    launched_by   TEXT    NOT NULL DEFAULT '',
    overrides     TEXT,
    runner_state  TEXT,
    outcome       TEXT,
    tool_help     TEXT,
    runner        TEXT,
    owner_instructions TEXT,
    PRIMARY KEY (agent, id)
);
CREATE INDEX IF NOT EXISTS sessions_newest
    ON sessions (agent, started_at DESC, id DESC);
CREATE TABLE IF NOT EXISTS turns (
    agent   TEXT    NOT NULL,
    session TEXT    NOT NULL,
    seq     INTEGER NOT NULL,
    at      INTEGER NOT NULL DEFAULT 0,
    role    TEXT    NOT NULL DEFAULT '',
    json    TEXT    NOT NULL,
    PRIMARY KEY (agent, session, seq),
    FOREIGN KEY (agent, session) REFERENCES sessions (agent, id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS queue (
    agent   TEXT    NOT NULL,
    session TEXT    NOT NULL,
    seq     INTEGER NOT NULL,
    message TEXT    NOT NULL,
    images  TEXT,
    PRIMARY KEY (agent, session, seq),
    FOREIGN KEY (agent, session) REFERENCES sessions (agent, id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS questions (
    agent    TEXT    NOT NULL,
    session  TEXT    NOT NULL,
    id       TEXT    NOT NULL,
    asked_at INTEGER NOT NULL DEFAULT 0,
    deadline INTEGER,
    json     TEXT    NOT NULL,
    answer   TEXT,
    PRIMARY KEY (agent, session, id),
    FOREIGN KEY (agent, session) REFERENCES sessions (agent, id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS questions_open
    ON questions (asked_at, id) WHERE answer IS NULL;
CREATE TABLE IF NOT EXISTS goals (
    agent         TEXT    NOT NULL,
    session       TEXT    NOT NULL,
    id            TEXT    NOT NULL,
    text          TEXT    NOT NULL DEFAULT '',
    state         TEXT    NOT NULL DEFAULT 'open',
    set_by        TEXT    NOT NULL DEFAULT 'human',
    created_at    INTEGER NOT NULL DEFAULT 0,
    last_nudge_at INTEGER NOT NULL DEFAULT 0,
    nudges        INTEGER NOT NULL DEFAULT 0,
    closed_at     INTEGER,
    note          TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (agent, session, id),
    FOREIGN KEY (agent, session) REFERENCES sessions (agent, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS goals_by_id ON goals (id);
CREATE INDEX IF NOT EXISTS goals_open ON goals (created_at, id) WHERE state = 'open';
CREATE TABLE IF NOT EXISTS attachments (
    id         TEXT    NOT NULL PRIMARY KEY,
    agent      TEXT    NOT NULL DEFAULT '',
    session    TEXT    NOT NULL DEFAULT '',
    name       TEXT    NOT NULL DEFAULT '',
    media_type TEXT    NOT NULL DEFAULT '',
    size       INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS attachments_of_session ON attachments (agent, session);
";

/// Columns added to a table that already exists, applied after [`SCHEMA`].
///
/// SQLite has no `ADD COLUMN IF NOT EXISTS`, and `CREATE TABLE IF NOT EXISTS` is a no-op against a
/// store built before the column existed — so a plain schema bump reaches new machines and silently
/// skips every old one. Each statement here is run on its own and its error swallowed: the only one
/// it can raise on a healthy store is *duplicate column name*, which is this having already been
/// applied. A real failure surfaces at the first query that needs the column, which is where it can
/// be read, rather than as an open error every process has to survive.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE sessions ADD COLUMN outcome TEXT",
    "ALTER TABLE sessions ADD COLUMN tool_help TEXT",
    "ALTER TABLE sessions ADD COLUMN runner TEXT",
    // Kept deliberately, by a person. `NOT NULL DEFAULT 0` and not nullable: every conversation
    // written before this existed was one nobody had starred, which is exactly what 0 says.
    "ALTER TABLE sessions ADD COLUMN starred INTEGER NOT NULL DEFAULT 0",
    // A queued message can carry images now. JSON rather than a join table: the queue is read whole
    // or not at all, and a message waiting in line is not something anything queries *by* image.
    "ALTER TABLE queue ADD COLUMN images TEXT",
    // Who asked for the run. Empty on every session opened before this existed, and that is the
    // honest answer for them — nobody wrote down who started them, and guessing after the fact
    // would put a person's name on runs an agent spawned. See `SessionRecord::launched_by`.
    "ALTER TABLE sessions ADD COLUMN launched_by TEXT NOT NULL DEFAULT ''",
    // What the run replaced in its agent's definition. Nullable, and NULL is the answer for every
    // session ever opened: a run overrides nothing unless somebody said so at the launch. JSON for
    // the reason `outcome` is — the set of settings an engine takes is the engine's business, and a
    // column per dial would be a table mostly full of nulls. See `SessionRecord::overrides`.
    "ALTER TABLE sessions ADD COLUMN overrides TEXT",
    // The fleet node that opened this conversation's own extra system-prompt instructions, frozen
    // the moment it was created. NULL for every session opened before this existed, and for the
    // overwhelming majority ever after: a launch not driven by another node over the mesh sets
    // nothing here. See `SessionStore::freeze_owner_instructions`.
    "ALTER TABLE sessions ADD COLUMN owner_instructions TEXT",
];

// One connection per thread per database.
//
// Not an optimization: `Agents::sessions` builds a store once per agent *and* again per idle run,
// so a single listing constructs it several hundred times. Opening a connection each time would
// cost more than the directory walk this replaces. A thread-local is also all the pooling that is
// needed — the app answers requests on a bounded blocking pool, so each worker opens once and
// reuses it, and WAL lets those readers run concurrently.
thread_local! {
    static CONNS: RefCell<HashMap<PathBuf, Rc<Connection>>> = RefCell::new(HashMap::new());
}

/// The connection for `path` on this thread, opening (and preparing) it on first use.
///
/// # Errors
/// Returns open, pragma, and schema errors.
pub(super) fn conn(path: &Path) -> Result<Rc<Connection>> {
    CONNS.with(|cache| {
        if let Some(existing) = cache.borrow().get(path) {
            return Ok(Rc::clone(existing));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path).map_err(|e| sql_err("open", e))?;
        connection
            .execute_batch(PRAGMAS)
            .map_err(|e| sql_err("configure", e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| sql_err("create the schema in", e))?;
        for statement in MIGRATIONS {
            let _ = connection.execute(statement, []);
        }
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
pub(super) fn forget_connections() {
    CONNS.with(|cache| cache.borrow_mut().clear());
}

/// A stored error, named after what was being attempted.
pub(super) fn sql_err(doing: &str, e: rusqlite::Error) -> Error {
    Error::Session(format!("couldn't {doing} the session database: {e}"))
}

/// The moment, in unix milliseconds — the unit every timestamp in this store is in.
///
/// Shared rather than written per module: three tables stamp rows with it, and three copies of the
/// same five lines is exactly the shape `adi-mono indexer clones` exists to find.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
