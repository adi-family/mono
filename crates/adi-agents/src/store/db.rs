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
/// `turns` and `queue` cascade off `sessions`: deleting a session takes its messages with it in one
/// statement rather than three that could half-fail. The index on `sessions` is the listing's own
/// order — newest first, id as tiebreak — so the query that replaced the directory walk reads it
/// straight off the index without a sort.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS sessions (
    agent         TEXT    NOT NULL,
    id            TEXT    NOT NULL,
    backend       TEXT    NOT NULL DEFAULT '',
    cwd           TEXT    NOT NULL DEFAULT '',
    message       TEXT    NOT NULL DEFAULT '',
    started_at    INTEGER NOT NULL DEFAULT 0,
    last_activity INTEGER NOT NULL DEFAULT 0,
    hidden        INTEGER NOT NULL DEFAULT 0,
    runner_state  TEXT,
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
    PRIMARY KEY (agent, session, seq),
    FOREIGN KEY (agent, session) REFERENCES sessions (agent, id) ON DELETE CASCADE
);
";

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
