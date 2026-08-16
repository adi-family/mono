//! What is waiting to be said next: the `queue` table, one row per message, oldest first by `seq`.
//!
//! One turn runs at a time, so anything said while a session is still answering waits here. Stored
//! rather than held in the browser, and for the same reason the record is: you can queue three
//! things, close the tab, and come back to find them answered.
//!
//! Nothing here knows whether a turn is running — that is the runner's question. The store only
//! keeps the line and hands out its head when asked.
//!
//! # Two things ask for the head, and one of them is a live turn
//!
//! Waiting for the answer to land is the floor, not the rule. An engine that drives its own loop
//! can take the next message *while it is still working* — [`take_as_turn`] is that door, and it is
//! why the pop and the transcript write share one transaction: a message that has left the queue
//! but never reached the transcript is a message nobody will ever answer.
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

use super::attachments::Attachment;
use super::db::sql_err;

/// A message waiting its turn: what was typed, and whatever was attached to it.
///
/// A pair rather than a bare string because the images have to wait *with* the message. Queue the
/// text alone and a screenshot pasted alongside it either arrives on the wrong turn or does not
/// arrive at all — and the person who attached it has already watched it appear in their own
/// bubble.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueuedMessage {
    pub text: String,
    pub images: Vec<Attachment>,
}

impl QueuedMessage {
    /// A message with nothing attached — what almost every queued message is.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }
}

/// Put `message` at the back of the line, returning its **1-based** place in it.
///
/// One-based because the number is shown to the person who typed it: the message just queued is
/// "1st in line", and an empty queue that now holds one entry answers `1`.
///
/// # Errors
/// Returns the database error. A message that could not be written down was not queued, and the
/// sender has to hear that rather than wait for an answer that will never come.
pub(super) fn enqueue(
    conn: &Connection,
    agent: &str,
    id: &str,
    message: &str,
    images: &[Attachment],
) -> Result<usize> {
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
        "INSERT INTO queue (agent, session, seq, message, images) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![agent, id, seq, message, encode(images)],
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
pub(super) fn dequeue(
    conn: &Connection,
    agent: &str,
    id: &str,
) -> Result<Option<QueuedMessage>> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("take from the queue of", e))?;
    let Some(message) = take_head(&tx, agent, id)? else {
        return Ok(None);
    };
    tx.commit()
        .map_err(|e| sql_err("take from the queue of", e))?;
    Ok(Some(message))
}

/// Take the head of the line **and record it as a question in the same breath**, returning what was
/// taken. `None` when nothing is waiting.
///
/// For an engine that can hear a new message mid-answer: it is already inside a turn, so there is no
/// launch to hang the message on and nothing else will write it down. Pop and append therefore
/// commit together — either the message is out of the queue *and* in the transcript, or it is still
/// in the queue and gets offered again on the next round.
///
/// # Errors
/// Returns the database error, and [`Error::NotFound`](crate::error::Error::NotFound) when the
/// session has been deleted out from under the turn. Nothing is committed in either case, so the
/// message stays where it was.
pub(super) fn take_as_turn(
    conn: &Connection,
    agent: &str,
    id: &str,
) -> Result<Option<QueuedMessage>> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("take from the queue of", e))?;
    let Some(message) = take_head(&tx, agent, id)? else {
        return Ok(None);
    };
    super::transcript::insert(
        &tx,
        agent,
        id,
        super::transcript::user_turn_with(&message.text, message.images.clone()),
    )?;
    tx.commit()
        .map_err(|e| sql_err("take from the queue of", e))?;
    Ok(Some(message))
}

/// Remove the oldest message and hand it back, inside a transaction the caller owns and commits —
/// what both ways of taking one are built from. `None` when the line is empty.
fn take_head(tx: &Connection, agent: &str, id: &str) -> Result<Option<QueuedMessage>> {
    let head: Option<(i64, String, Option<String>)> = tx
        .query_row(
            "SELECT seq, message, images FROM queue WHERE agent = ?1 AND session = ?2
             ORDER BY seq LIMIT 1",
            [agent, id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| sql_err("take from the queue of", e))?;
    let Some((seq, text, images)) = head else {
        return Ok(None);
    };
    tx.execute(
        "DELETE FROM queue WHERE agent = ?1 AND session = ?2 AND seq = ?3",
        rusqlite::params![agent, id, seq],
    )
    .map_err(|e| sql_err("take from the queue of", e))?;
    Ok(Some(QueuedMessage {
        text,
        images: decode(images.as_deref()),
    }))
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
pub(super) fn load(conn: &Connection, agent: &str, id: &str) -> Vec<QueuedMessage> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT message, images FROM queue WHERE agent = ?1 AND session = ?2 ORDER BY seq",
    ) else {
        return Vec::new();
    };
    stmt.query_map([agent, id], |row| {
        Ok(QueuedMessage {
            text: row.get(0)?,
            images: decode(row.get::<_, Option<String>>(1)?.as_deref()),
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// The attachments column, as it is written. `NULL` for a message with none — the column was added
/// to a table that already had rows in it, and every one of those predates images.
fn encode(images: &[Attachment]) -> Option<String> {
    (!images.is_empty()).then(|| serde_json::to_string(images).unwrap_or_default())
}

/// The attachments column, as it is read. Unparseable is the same as absent: the queue's job is to
/// deliver the message, and a row whose images cannot be decoded still has words worth asking.
fn decode(json: Option<&str>) -> Vec<Attachment> {
    json.and_then(|raw| serde_json::from_str(raw).ok())
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
