//! What is waiting to be said next: the `queue` table, one row per message, oldest first by `seq`.
//!
//! One turn runs at a time, so anything said while a session is still answering waits here. Stored
//! rather than held in the browser, and for the same reason the record is: you can queue three
//! things, close the tab, and come back to find them answered.
//!
//! Nothing here knows whether a turn is running — that is the runner's question. The store only
//! keeps the line and hands out its head when asked.
//!
//! # The lock is the database's now
//!
//! Every edit below is a read-modify-write, and the callers are concurrent by construction: an app
//! server answering requests in parallel, an open chat polling twice a second, and a CLI in another
//! process entirely. This used to be guarded by a process-global `Mutex`, which covered the first
//! two and did nothing whatever about the third — two processes could each read the head, each act
//! on it, and the same message be asked twice. A transaction covers all three, because the lock is
//! held by the file rather than by one process's memory.

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;

use super::db::sql_err;

/// Put `message` at the back of the line, returning its **1-based** place in it.
///
/// One-based because the number is shown to the person who typed it: the message just queued is
/// "1st in line", and an empty queue that now holds one entry answers `1`.
///
/// # Errors
/// Returns the database error. A message that could not be written down was not queued, and the
/// sender has to hear that rather than wait for an answer that will never come.
pub(super) fn enqueue(conn: &Connection, agent: &str, id: &str, message: &str) -> Result<usize> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("queue a message in", e))?;
    let seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM queue WHERE agent = ?1 AND session = ?2",
            [agent, id],
            |row| row.get(0),
        )
        .map_err(|e| sql_err("queue a message in", e))?;
    tx.execute(
        "INSERT INTO queue (agent, session, seq, message) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![agent, id, seq, message],
    )
    .map_err(|e| sql_err("queue a message in", e))?;
    let place: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM queue WHERE agent = ?1 AND session = ?2",
            [agent, id],
            |row| row.get(0),
        )
        .map_err(|e| sql_err("queue a message in", e))?;
    tx.commit().map_err(|e| sql_err("queue a message in", e))?;
    Ok(usize::try_from(place).unwrap_or(0))
}

/// Take the head of the line, or `None` when nothing is waiting.
///
/// The removal is committed *before* the message is handed back, so a message that fails to launch
/// has still had its turn. Leaving it at the head would retry it on every poll for ever.
///
/// # Errors
/// Returns the database error, with the queue left as it was — the caller must not start a turn
/// whose removal was not recorded, or the same thing is asked twice.
pub(super) fn dequeue(conn: &Connection, agent: &str, id: &str) -> Result<Option<String>> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("take from the queue of", e))?;
    let head: Option<(i64, String)> = tx
        .query_row(
            "SELECT seq, message FROM queue WHERE agent = ?1 AND session = ?2
             ORDER BY seq LIMIT 1",
            [agent, id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| sql_err("take from the queue of", e))?;
    let Some((seq, message)) = head else {
        return Ok(None);
    };
    tx.execute(
        "DELETE FROM queue WHERE agent = ?1 AND session = ?2 AND seq = ?3",
        rusqlite::params![agent, id, seq],
    )
    .map_err(|e| sql_err("take from the queue of", e))?;
    tx.commit()
        .map_err(|e| sql_err("take from the queue of", e))?;
    Ok(Some(message))
}

/// How many messages are waiting.
pub(super) fn len(conn: &Connection, agent: &str, id: &str) -> usize {
    conn.query_row(
        "SELECT COUNT(*) FROM queue WHERE agent = ?1 AND session = ?2",
        [agent, id],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| usize::try_from(n).unwrap_or(0))
    .unwrap_or(0)
}

/// The messages waiting their turn, oldest first.
pub(super) fn load(conn: &Connection, agent: &str, id: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT message FROM queue WHERE agent = ?1 AND session = ?2 ORDER BY seq",
    ) else {
        return Vec::new();
    };
    stmt.query_map([agent, id], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Drop the message at `index` (0-based, in the order they will be asked), for something you have
/// thought better of. Returns whether there was one there to drop — an index past the end is a
/// stale click, not an error.
///
/// # Errors
/// Returns the database error.
pub(super) fn unqueue(conn: &Connection, agent: &str, id: &str, index: usize) -> Result<bool> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("drop a queued message from", e))?;
    // By position rather than by `seq`: the caller counted rows in a view, and gaps left by earlier
    // removals mean the two stopped agreeing long ago.
    let seq: Option<i64> = tx
        .query_row(
            "SELECT seq FROM queue WHERE agent = ?1 AND session = ?2
             ORDER BY seq LIMIT 1 OFFSET ?3",
            rusqlite::params![agent, id, i64::try_from(index).unwrap_or(i64::MAX)],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| sql_err("drop a queued message from", e))?;
    let Some(seq) = seq else {
        return Ok(false);
    };
    tx.execute(
        "DELETE FROM queue WHERE agent = ?1 AND session = ?2 AND seq = ?3",
        rusqlite::params![agent, id, seq],
    )
    .map_err(|e| sql_err("drop a queued message from", e))?;
    tx.commit()
        .map_err(|e| sql_err("drop a queued message from", e))?;
    Ok(true)
}

/// Forget everything waiting — what stopping the current answer does. A queued message was written
/// expecting the answer you just cut short, so it goes with it.
///
/// # Errors
/// Returns the database error.
pub(super) fn clear(conn: &Connection, agent: &str, id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM queue WHERE agent = ?1 AND session = ?2",
        [agent, id],
    )
    .map_err(|e| sql_err("clear the queue of", e))?;
    Ok(())
}
