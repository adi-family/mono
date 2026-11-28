//! What a session said: the `turns` table, one row per turn, oldest first by `seq`.
//!
//! A runner never sees this. It answers one turn at a time and knows nothing about the ones before
//! it; what the session *is* — the sequence of questions and answers a reader scrolls through — is
//! durable, so it belongs here with the record and the queue.
//!
//! Append-only, with one exception. A turn that has landed is final, and two writers appending
//! never collide because the sequence number is taken inside the same transaction as the insert.
//! The exception is [`truncate`]: a reader rewinding the conversation to one of their own messages
//! drops that turn and everything after it, and the numbers above the cut are minted again by
//! whatever is said next.
//!
//! # Three kinds of turn, only one of them recorded
//!
//! What a reader renders is the recorded turns plus two things that exist nowhere but the view:
//!
//! - the **pending** answer, still being written — synthesized from the live content the caller
//!   hands in, so the answer streams into the view before it is committed;
//! - the **queued** questions, said but not yet asked — synthesized from the queue, in the order
//!   they will be put.
//!
//! Neither is ever written down. A pending turn is by definition not final, and a queued message
//! becomes a real user turn only when its turn starts — persisting either would leave a duplicate
//! behind the moment it became real.
//!
//! # The store does not parse engine output
//!
//! The live content arrives as an already-parsed [`TurnContent`], produced by the runner whose
//! format it is. Nothing here knows what a `stream-json` event or an ADI loop event looks like, and
//! that is the whole point of the layering: adding an engine must not mean teaching the store a
//! wire format.

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::marker::Marker;
use crate::progress::{Step, TurnContent, TurnMetrics, close_open_calls};

use super::attachments::Attachment;
use super::db::{now_ms, sql_err};
use super::queue::QueueMode;

/// One message in a session's transcript.
///
/// Engine-agnostic on purpose: a turn is a role, some text, and what the engine reported doing to
/// produce it. Every backend's output is normalized into this shape by its own runner, so a reader
/// renders a `process:*` run and an `adi` conversation through one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// `"user"` or `"assistant"`.
    pub role: String,
    pub text: String,
    /// Unix milliseconds the turn was recorded.
    #[serde(default)]
    pub at: u64,
    /// True only for the provisional, still-streaming assistant turn synthesized from the live log —
    /// never recorded (a committed turn is settled and final).
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending: bool,
    /// True only for a user message still waiting in the queue — said, but not yet asked.
    /// Synthesized on read; it becomes a real transcript turn when its turn starts.
    #[serde(default, skip_serializing_if = "is_false")]
    pub queued: bool,
    /// When a still-[`queued`](Self::queued) message wants to be heard — carried onto the
    /// synthesized turn straight from the [`QueuedMessage`](super::QueuedMessage) it came from, so
    /// a reader can say "asap" before the message is ever asked. Meaningless once `queued` is
    /// false: a committed turn has already been heard, whatever it was queued as.
    #[serde(default, skip_serializing_if = "is_regular")]
    pub mode: QueueMode,
    /// The images this message carries — a user turn's attachments, in the order they were
    /// attached. References rather than bytes (see [`Attachment`]), so a transcript polled once a
    /// second stays the size of its text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Attachment>,
    /// The assistant turn's activity — tool calls and thinking — parsed from the engine's output.
    /// Empty for user turns and for engines that emit no structured progress.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// The assistant turn's telemetry (tokens / cost / duration), when the engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TurnMetrics>,
    /// What the platform stamped on this message — who sent it, and which wake, answer or nudge it
    /// *is*. Only ever on a user turn, and more than one when more than one is true of it.
    ///
    /// Data rather than words in [`text`](Self::text), on the same rule as the images and the
    /// pre-run steps beside it: the transcript keeps the message a person actually wrote, and
    /// [`marker::stamp`](crate::marker::stamp) puts the tags back on the way to the engine. Empty
    /// on every turn nobody stamped — and on every turn recorded before this field existed, whose
    /// markers are still in its text and are read back out of it by
    /// [`marker::split`](crate::marker::split).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
}

/// Taking `&bool` is what serde's `skip_serializing_if` requires — it hands the predicate a
/// reference to the field, so the by-value form clippy asks for cannot be named there.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// The same requirement as [`is_false`], for [`Turn::mode`] — regular is nearly every turn there
/// is, and every turn that was never queued at all.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_regular(mode: &QueueMode) -> bool {
    *mode == QueueMode::Regular
}

/// The two roles a turn carries. Strings rather than an enum because [`Turn::role`] is one, and it
/// crosses the wire to readers that already expect these exact words.
pub(crate) const ROLE_USER: &str = "user";
pub(crate) const ROLE_ASSISTANT: &str = "assistant";

/// A question, ready to append. What the store records when a message is actually put to the agent —
/// not when it was typed, which is what the queue is for.
#[must_use]
pub fn user_turn(text: impl Into<String>) -> Turn {
    user_turn_with(text, Vec::new())
}

/// The same question, carrying the images that were attached to it.
///
/// Separate from [`user_turn`] rather than replacing it because most questions have none, and a
/// caller that has to pass an empty vec to say "no pictures" is a caller that will pass one by
/// mistake somewhere it mattered.
#[must_use]
pub fn user_turn_with(text: impl Into<String>, images: Vec<Attachment>) -> Turn {
    Turn {
        role: ROLE_USER.to_string(),
        text: text.into(),
        at: now_ms(),
        pending: false,
        queued: false,
        mode: QueueMode::Regular,
        images,
        steps: Vec::new(),
        metrics: None,
        markers: Vec::new(),
    }
}

/// A finished answer, ready to append: its text, the timeline of what it did to get there, and
/// whatever telemetry the engine reported.
#[must_use]
pub fn assistant_turn(content: &TurnContent) -> Turn {
    Turn {
        role: ROLE_ASSISTANT.to_string(),
        text: content.text.clone(),
        at: now_ms(),
        pending: false,
        queued: false,
        mode: QueueMode::Regular,
        // An answer carries none: what a model sends back is text and the steps it took to write
        // it. Images travel one way, from the person to the model.
        images: Vec::new(),
        steps: content.steps.clone(),
        metrics: content.metrics.clone(),
        markers: Vec::new(),
    }
}

/// Record one turn, and move the session's `last_activity` with it.
///
/// The two view-only flags are cleared on the way in. A caller holding a turn it took *from* a view
/// (the pending answer it has just watched finish, say) would otherwise commit it still flagged, and
/// a recorded `pending` turn is a contradiction that never settles: every later read would splice a
/// second pending answer in beside it. The recorded moment is filled in for the same reason — a
/// stored turn always says when it landed, whether or not its author remembered to.
///
/// A still-running tool call is closed on the way in for exactly that reason: a recorded turn is
/// final, so nothing in it can still be happening. A turn killed mid-call is the ordinary way one
/// arrives here, and left alone it would sit "running" in a finished conversation for ever.
///
/// All of it in one transaction: the sequence number is read and used under the same lock, so two
/// processes appending at once cannot mint the same one, and a session's activity can never
/// disagree with its last turn.
///
/// # Errors
/// Returns [`Error::NotFound`] when the session is gone, and database errors.
pub(super) fn append(conn: &Connection, agent: &str, id: &str, turn: Turn) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("record a turn in", e))?;
    insert(&tx, agent, id, turn)?;
    tx.commit().map_err(|e| sql_err("record a turn in", e))?;
    Ok(())
}

/// The body of [`append`], inside a transaction the caller owns and commits.
///
/// Split out for the one caller that must record a turn and do something else in the *same* commit:
/// [`super::queue::take_as_turn`], which moves a message from the queue into the transcript. Two
/// transactions there would leave a window where a crash loses the message from both.
///
/// # Errors
/// Returns [`Error::NotFound`] when the session is gone, and database errors. The caller's
/// transaction is left uncommitted, so an error here records nothing.
///
/// The platform events recorded while this turn was in flight ([`record_event`]) are folded into
/// its steps and their rows deleted in the same commit — here rather than in the one caller that
/// settles an answer, because every path that records a turn takes that turn's `seq`, and rows
/// left behind for a `seq` already taken would never be read again.
pub(super) fn insert(tx: &Connection, agent: &str, id: &str, mut turn: Turn) -> Result<()> {
    turn.pending = false;
    turn.queued = false;
    if turn.at == 0 {
        turn.at = now_ms();
    }

    let known: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM sessions WHERE agent = ?1 AND id = ?2",
            [agent, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| sql_err("record a turn in", e))?;
    if known.is_none() {
        return Err(Error::NotFound(format!("{agent}: no session {id}")));
    }
    let seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM turns WHERE agent = ?1 AND session = ?2",
            [agent, id],
            |row| row.get(0),
        )
        .map_err(|e| sql_err("record a turn in", e))?;
    let events = events_for(tx, agent, id, seq);
    if !events.is_empty() {
        turn.steps = splice_events(std::mem::take(&mut turn.steps), events);
        tx.execute(
            "DELETE FROM turn_events WHERE agent = ?1 AND session = ?2 AND turn_seq = ?3",
            rusqlite::params![agent, id, seq],
        )
        .map_err(|e| sql_err("record a turn in", e))?;
    }
    close_open_calls(&mut turn.steps);
    let json = serde_json::to_string(&turn)
        .map_err(|e| Error::Session(format!("couldn't encode a turn of session {id}: {e}")))?;
    tx.execute(
        "INSERT INTO turns (agent, session, seq, at, role, json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![agent, id, seq, turn.at, turn.role, json],
    )
    .map_err(|e| sql_err("record a turn in", e))?;
    // Never earlier than the start, which is what makes this always a moment the session existed.
    tx.execute(
        "UPDATE sessions SET last_activity = MAX(?3, started_at) WHERE agent = ?1 AND id = ?2",
        rusqlite::params![agent, id, turn.at],
    )
    .map_err(|e| sql_err("record a turn in", e))?;
    // Recording the turn is what makes its images this conversation's — in the same commit, so an
    // attachment is never claimed by a turn that was not written. Until now they were nobody's, and
    // the unclaimed sweep was free to take them.
    let ids: Vec<String> = turn.images.iter().map(|image| image.id.clone()).collect();
    super::attachments::claim(tx, agent, id, &ids);
    Ok(())
}

/// Drop the turn numbered `from` and every turn after it, returning how many went.
///
/// A question still open that was asked in those turns goes with them: the agent that asked it no
/// longer remembers asking, and an open question is what holds a chat's composer shut. One asked
/// earlier stays — it is still what the kept conversation is waiting on.
///
/// The two notification marks are cleared with them. Each remembers the turn *count* it last fired
/// at, and a conversation regrown to that same count is at a different turn — left set, its next
/// idle would be taken for one already announced.
///
/// # Errors
/// Returns database errors.
pub(super) fn truncate(conn: &Connection, agent: &str, id: &str, from: usize) -> Result<usize> {
    let from = i64::try_from(from)
        .map_err(|_| Error::Session(format!("turn {from} of session {id} is out of range")))?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("rewind", e))?;
    tx.execute(
        "DELETE FROM questions
         WHERE agent = ?1 AND session = ?2 AND answer IS NULL AND asked_at >=
               (SELECT at FROM turns WHERE agent = ?1 AND session = ?2 AND seq = ?3)",
        rusqlite::params![agent, id, from],
    )
    .map_err(|e| sql_err("rewind", e))?;
    let dropped = tx
        .execute(
            "DELETE FROM turns WHERE agent = ?1 AND session = ?2 AND seq >= ?3",
            rusqlite::params![agent, id, from],
        )
        .map_err(|e| sql_err("rewind", e))?;
    tx.execute(
        "DELETE FROM turn_events WHERE agent = ?1 AND session = ?2 AND turn_seq >= ?3",
        rusqlite::params![agent, id, from],
    )
    .map_err(|e| sql_err("rewind", e))?;
    tx.execute(
        "UPDATE sessions SET idle_notified_turn = NULL, lease_wait_notified_turn = NULL
         WHERE agent = ?1 AND id = ?2",
        [agent, id],
    )
    .map_err(|e| sql_err("rewind", e))?;
    tx.commit().map_err(|e| sql_err("rewind", e))?;
    Ok(dropped)
}

/// The recorded turns, oldest first.
///
/// A row that will not decode is skipped rather than failing the read — one torn turn must not take
/// a whole conversation out of the view.
///
/// Open calls are closed here as well as on the way in, because the rows written before that was
/// true are still on disk, and a conversation from last week does not get a second chance to be
/// recorded correctly. Closing on read is what heals them.
pub(super) fn load(conn: &Connection, agent: &str, id: &str) -> Vec<Turn> {
    let Ok(mut stmt) = conn
        .prepare_cached("SELECT json FROM turns WHERE agent = ?1 AND session = ?2 ORDER BY seq")
    else {
        return Vec::new();
    };
    stmt.query_map([agent, id], |row| row.get::<_, String>(0))
        .map(|rows| {
            rows.flatten()
                .filter_map(|json| serde_json::from_str::<Turn>(&json).ok())
                .map(|mut turn| {
                    close_open_calls(&mut turn.steps);
                    turn
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A page of a transcript: the turns themselves, each with its place in the whole conversation,
/// and how long that conversation is.
///
/// The place travels with the turn because a page cannot be counted from where it sits: the reader
/// of the newest twenty turns of a hundred still has to address turn 83 as 83 — its anchors, the
/// steps it asks to be expanded, and the rail's links into it are all built from that number. It is
/// the row's own `seq`, not an index into [`turns`](Self::turns), for the same reason
/// [`load`] skips a row it cannot decode: counting positions would renumber the conversation
/// around a torn row, silently, once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptPage {
    /// The window, oldest first, each turn paired with its place in the conversation.
    pub turns: Vec<(usize, Turn)>,
    /// How many turns the conversation has in all — the synthesized ones (the answer being written,
    /// the questions still queued) included, because they are turns to a reader. What a client
    /// subtracts from to say how many are still unread.
    pub total: usize,
    /// How many of those are **recorded** — rows, and so final.
    ///
    /// The line between what can be remembered about a conversation and what has to be looked at
    /// again: turns below this are append-only and never change, and everything above is the answer
    /// being written and the queue behind it. `total - recorded` is how many of the latter there
    /// are.
    pub recorded: usize,
}

/// The recorded turns of one page, oldest first, each with its own `seq`.
///
/// `before` is exclusive and `None` means "the newest": the query runs backwards from there and the
/// result is flipped, which is how the *last* N rows are read without walking the first thousand.
///
/// A row that will not decode is skipped, exactly as [`load`] skips it — and its `seq` goes with it,
/// so the turns around it keep the numbers they have always had.
pub(super) fn load_page(
    conn: &Connection,
    agent: &str,
    id: &str,
    before: Option<usize>,
    limit: usize,
) -> Vec<(usize, Turn)> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT seq, json FROM turns WHERE agent = ?1 AND session = ?2 AND seq < ?3
         ORDER BY seq DESC LIMIT ?4",
    ) else {
        return Vec::new();
    };
    // `i64` is what the column is, and `usize::MAX` would overflow it. Nothing has i64::MAX turns.
    let before = i64::try_from(before.unwrap_or(usize::MAX)).unwrap_or(i64::MAX);
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut page = stmt
        .query_map(rusqlite::params![agent, id, before, limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map(|rows| {
            rows.flatten()
                .filter_map(|(seq, json)| {
                    let mut turn = serde_json::from_str::<Turn>(&json).ok()?;
                    close_open_calls(&mut turn.steps);
                    Some((usize::try_from(seq).unwrap_or(0), turn))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    page.reverse();
    page
}

/// The newest recorded turn, or `None` for a conversation that has not said anything.
///
/// One row, by `ORDER BY seq DESC LIMIT 1`. It exists because the lazy clock that commits a
/// finished answer ([`crate::settle`]) asks one question of a transcript — "was the last thing said
/// a question?" — and used to load the whole of it to answer: measured at ~180ms per read on a
/// 3.4 MB conversation, on the path a watched chat takes **once a second**.
///
/// A torn newest row answers `None` rather than the newest *readable* turn. That is the safer
/// difference: what this decides is whether to append an answer, and appending one behind a turn
/// nothing can read would be guessing at what it was.
pub(super) fn last(conn: &Connection, agent: &str, id: &str) -> Option<Turn> {
    let json: String = conn
        .query_row(
            "SELECT json FROM turns WHERE agent = ?1 AND session = ?2 ORDER BY seq DESC LIMIT 1",
            [agent, id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    serde_json::from_str::<Turn>(&json).ok()
}

/// One recorded turn by its `seq`, healed on the way out exactly as [`load`] heals a page.
pub(super) fn one(conn: &Connection, agent: &str, id: &str, seq: usize) -> Option<Turn> {
    let seq = i64::try_from(seq).ok()?;
    let json: String = conn
        .query_row(
            "SELECT json FROM turns WHERE agent = ?1 AND session = ?2 AND seq = ?3",
            rusqlite::params![agent, id, seq],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let mut turn = serde_json::from_str::<Turn>(&json).ok()?;
    close_open_calls(&mut turn.steps);
    Some(turn)
}

/// How many turns are recorded. The number a page is a window onto, counted in SQL rather than by
/// loading what it counts.
pub(super) fn count(conn: &Connection, agent: &str, id: &str) -> usize {
    conn.query_row(
        "SELECT COUNT(*) FROM turns WHERE agent = ?1 AND session = ?2",
        [agent, id],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| usize::try_from(n).unwrap_or(0))
    .unwrap_or(0)
}

/// Settle `turn` as the answer to the question the conversation ends on — or do nothing, returning
/// `false`, when the last turn is not a question (somebody else settled it first).
///
/// The check and the write share one transaction, which is what lets [`record_event`] decide
/// "in flight or settled" without an event slipping between the two.
///
/// # Errors
/// Returns [`Error::NotFound`] when the session is gone, and database errors.
pub(super) fn answer(conn: &Connection, agent: &str, id: &str, turn: Turn) -> Result<bool> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("record a turn in", e))?;
    let last_role: Option<String> = tx
        .query_row(
            "SELECT role FROM turns WHERE agent = ?1 AND session = ?2 ORDER BY seq DESC LIMIT 1",
            [agent, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| sql_err("record a turn in", e))?;
    if last_role.as_deref() != Some(ROLE_USER) {
        return Ok(false);
    }
    insert(&tx, agent, id, turn)?;
    tx.commit().map_err(|e| sql_err("record a turn in", e))?;
    Ok(true)
}

/// Record a platform event step onto a conversation.
///
/// While a question is unanswered the event waits in `turn_events`, keyed to the `seq` its answer
/// will take and pinned `after_step` steps into the engine's stream; [`insert`] folds it in when
/// that answer is recorded. Otherwise it is appended to the newest turn.
///
/// Deciding which, and writing, happen in one transaction — and every write transaction here is
/// `IMMEDIATE` (see `db::conn`) — so an event is never filed under an answer that was settled
/// between the look and the write.
///
/// # Errors
/// Returns [`Error::NotFound`] when the session is gone or has no turns, and database errors.
pub(super) fn record_event(
    conn: &Connection,
    agent: &str,
    id: &str,
    after_step: usize,
    step: Step,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("record an event in", e))?;
    let known: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM sessions WHERE agent = ?1 AND id = ?2",
            [agent, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| sql_err("record an event in", e))?;
    if known.is_none() {
        return Err(Error::NotFound(format!("{agent}: no session {id}")));
    }
    let last: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT seq, role, json FROM turns WHERE agent = ?1 AND session = ?2
             ORDER BY seq DESC LIMIT 1",
            [agent, id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| sql_err("record an event in", e))?;
    let Some((seq, role, json)) = last else {
        return Err(Error::NotFound(format!(
            "{agent}: no turns in session {id}"
        )));
    };
    if role == ROLE_USER {
        let json = serde_json::to_string(&step).map_err(|e| {
            Error::Session(format!("couldn't encode an event for session {id}: {e}"))
        })?;
        let after_step = i64::try_from(after_step).unwrap_or(i64::MAX);
        tx.execute(
            "INSERT INTO turn_events (agent, session, turn_seq, seq, after_step, json)
             SELECT ?1, ?2, ?3, COALESCE(MAX(seq), -1) + 1, ?4, ?5 FROM turn_events
             WHERE agent = ?1 AND session = ?2 AND turn_seq = ?3",
            rusqlite::params![agent, id, seq + 1, after_step, json],
        )
        .map_err(|e| sql_err("record an event in", e))?;
    } else {
        let mut turn: Turn = serde_json::from_str(&json).map_err(|e| {
            Error::Session(format!("couldn't decode turn {seq} of session {id}: {e}"))
        })?;
        turn.steps.push(step);
        let updated = serde_json::to_string(&turn).map_err(|e| {
            Error::Session(format!("couldn't encode turn {seq} of session {id}: {e}"))
        })?;
        tx.execute(
            "UPDATE turns SET json = ?4 WHERE agent = ?1 AND session = ?2 AND seq = ?3",
            rusqlite::params![agent, id, seq, updated],
        )
        .map_err(|e| sql_err("record an event in", e))?;
    }
    tx.commit().map_err(|e| sql_err("record an event in", e))?;
    Ok(())
}

/// The events waiting for the answer still being written: one query, the turn's `seq` worked out
/// in SQL rather than by a second round trip for the count.
pub(super) fn live_events(conn: &Connection, agent: &str, id: &str) -> Vec<(usize, Step)> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT after_step, json FROM turn_events
         WHERE agent = ?1 AND session = ?2
           AND turn_seq = (SELECT COALESCE(MAX(seq), -1) + 1 FROM turns
                           WHERE agent = ?1 AND session = ?2)
         ORDER BY after_step ASC, seq ASC",
    ) else {
        return Vec::new();
    };
    decode_events(stmt.query_map([agent, id], event_row))
}

/// The events waiting for turn `turn_seq`, in the order [`splice_events`] wants them.
fn events_for(conn: &Connection, agent: &str, id: &str, turn_seq: i64) -> Vec<(usize, Step)> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT after_step, json FROM turn_events
         WHERE agent = ?1 AND session = ?2 AND turn_seq = ?3
         ORDER BY after_step ASC, seq ASC",
    ) else {
        return Vec::new();
    };
    decode_events(stmt.query_map(rusqlite::params![agent, id, turn_seq], event_row))
}

fn event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, String)> {
    Ok((row.get(0)?, row.get(1)?))
}

fn decode_events<I>(rows: rusqlite::Result<I>) -> Vec<(usize, Step)>
where
    I: Iterator<Item = rusqlite::Result<(i64, String)>>,
{
    rows.map(|rows| {
        rows.flatten()
            .filter_map(|(after_step, json)| {
                let step = serde_json::from_str::<Step>(&json).ok()?;
                Some((usize::try_from(after_step).unwrap_or(0), step))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Interleave platform events at their recorded `after_step` positions in the engine's steps; an
/// event pinned past the end (the engine's stream is shorter than when it was recorded) goes last.
pub(crate) fn splice_events(engine_steps: Vec<Step>, events: Vec<(usize, Step)>) -> Vec<Step> {
    let mut merged = Vec::with_capacity(engine_steps.len() + events.len());
    let mut events = events.into_iter().peekable();
    for (i, step) in engine_steps.into_iter().enumerate() {
        while let Some((_, event)) = events.next_if(|(at, _)| *at <= i) {
            merged.push(event);
        }
        merged.push(step);
    }
    merged.extend(events.map(|(_, event)| event));
    merged
}

/// [`view`], for a page: the same splice, over a window that may not reach the end of the
/// conversation.
///
/// The two synthesized kinds are appended **only to the last page** (`tail`), because that is where
/// they are: an answer being written is behind the newest question, and a queued message is behind
/// everything. Paging backwards for older turns must not drag them along — a reader would see the
/// live answer twice, once in its place and once at the foot of every page they opened.
///
/// `recorded` is the whole conversation's turn count, not the page's: the synthesized turns are
/// numbered from the end of the *conversation*, so the answer being written is turn N whether the
/// reader is looking at the newest page or the first.
pub(super) fn page_view(
    turns: Vec<(usize, Turn)>,
    recorded: usize,
    tail: bool,
    live: Option<TurnContent>,
    running: bool,
    queued: Vec<super::queue::QueuedMessage>,
) -> TranscriptPage {
    let mut page = TranscriptPage {
        turns,
        total: recorded,
        recorded,
    };
    if !tail {
        return page;
    }
    // Anchored on the last *recorded* turn, as [`view`] is — but read off the page, which on the
    // tail always ends with it.
    let answering = page.turns.last().map(|(_, t)| t.role.as_str()) == Some(ROLE_USER);
    if let Some(content) = live
        && answering
    {
        let mut steps = content.steps;
        if !running {
            close_open_calls(&mut steps);
        }
        page.turns.push((
            page.total,
            Turn {
                role: ROLE_ASSISTANT.to_string(),
                text: content.text,
                at: 0,
                pending: running,
                queued: false,
                mode: QueueMode::Regular,
                images: Vec::new(),
                steps,
                metrics: content.metrics,
                markers: Vec::new(),
            },
        ));
        page.total += 1;
    }
    for message in queued {
        page.turns.push((
            page.total,
            Turn {
                role: ROLE_USER.to_string(),
                text: message.text,
                at: 0,
                pending: false,
                queued: true,
                mode: message.mode,
                images: message.images,
                steps: Vec::new(),
                metrics: None,
                markers: message.markers,
            },
        ));
        page.total += 1;
    }
    page
}

/// The full view: what was recorded, what is being said right now, and what is still waiting.
///
/// `live` is spliced in **only behind an unanswered question**. A reader polls, so it hands in the
/// current parse of the in-flight answer on every call — including the ones after that answer has
/// been committed, which it has no way to notice. Anchoring on the last recorded turn means the
/// same content shows once as a pending answer and then once as a settled one, never as both.
///
/// `running` decides only the flag, not the splice: an answer whose child has exited but which
/// nobody has committed yet is still the truest thing to show for that question — it just is not
/// streaming any more. Which is also why it decides whether that answer's calls stay open: nothing
/// is left to answer them once the child is gone.
pub(super) fn view(
    mut turns: Vec<Turn>,
    live: Option<TurnContent>,
    running: bool,
    queued: Vec<super::queue::QueuedMessage>,
) -> Vec<Turn> {
    if let Some(content) = live
        && turns.last().map(|t| t.role.as_str()) == Some(ROLE_USER)
    {
        let mut steps = content.steps;
        if !running {
            close_open_calls(&mut steps);
        }
        turns.push(Turn {
            role: ROLE_ASSISTANT.to_string(),
            text: content.text,
            // No recorded moment: nothing has been recorded. It gets one when it is appended.
            at: 0,
            pending: running,
            queued: false,
            mode: QueueMode::Regular,
            images: Vec::new(),
            steps,
            metrics: content.metrics,
            markers: Vec::new(),
        });
    }
    turns.extend(queued.into_iter().map(|message| Turn {
        role: ROLE_USER.to_string(),
        text: message.text,
        at: 0,
        pending: false,
        queued: true,
        mode: message.mode,
        images: message.images,
        steps: Vec::new(),
        metrics: None,
        // Carried onto the synthesized turn so a queued message reads as what it is before it is
        // ever asked: the chat draws its bubble from this, minutes before the turn exists.
        markers: message.markers,
    }));
    turns
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;
    use crate::progress::ToolStatus;
    use crate::store::SessionStore;

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-transcript-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        super::super::db::forget_connections();
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    #[test]
    fn regression_appending_waits_for_another_sessions_writer() {
        use std::cell::RefCell;
        use std::sync::mpsc::{Receiver, Sender, channel};
        use std::time::Duration;

        enum Attempt {
            Waiting,
            Finished(std::result::Result<(), String>),
        }
        thread_local! {
            static RETRY: RefCell<Option<(Sender<Attempt>, Receiver<()>)>> = const { RefCell::new(None) };
        }
        fn retry_after_writer_exits(_: i32) -> bool {
            RETRY.with(|retry| {
                let retry = retry.borrow();
                let (events, release) = retry.as_ref().expect("installed on this worker");
                let _ = events.send(Attempt::Waiting);
                release.recv_timeout(Duration::from_secs(5)).is_ok()
            })
        }

        let store = scratch("writer-contention");
        let first = store
            .create("chat", Backend::HarnessAdi, "/tmp", "first")
            .unwrap();
        let second = store
            .create("chat", Backend::HarnessAdi, "/tmp", "second")
            .unwrap();
        let conn = super::super::db::conn(&store.db_path()).unwrap();
        let (ready_tx, ready_rx) = channel();
        let (start_tx, start_rx) = channel();
        let (events_tx, events_rx) = channel();
        let (release_tx, release_rx) = channel();
        let worker_store = store.clone();
        let second_id = second.id.clone();
        let worker = std::thread::spawn(move || {
            // Open before the other connection takes its lock: schema initialization is not
            // the operation under test. The busy handler releases the writer without sleeps.
            RETRY.with(|retry| *retry.borrow_mut() = Some((events_tx.clone(), release_rx)));
            super::super::db::configure_connection(&worker_store.db_path(), |conn| {
                conn.busy_handler(Some(retry_after_writer_exits))
            })
            .unwrap();
            ready_tx.send(()).unwrap();
            start_rx.recv().unwrap();
            let result = worker_store
                .append_turn("chat", &second_id, user_turn("must survive contention"))
                .map_err(|error| error.to_string());
            events_tx.send(Attempt::Finished(result)).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "UPDATE sessions SET title = 'writing' WHERE id = ?1",
            [&first.id],
        )
        .unwrap();
        start_tx.send(()).unwrap();
        let attempt = events_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        conn.execute_batch("COMMIT").unwrap();
        let _ = release_tx.send(());
        let result = match attempt {
            Attempt::Finished(result) => result,
            Attempt::Waiting => match events_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Attempt::Finished(result) => result,
                Attempt::Waiting => panic!("the writer has already released its lock"),
            },
        };
        worker.join().unwrap();
        result.expect("a different conversation's short write must not reject this turn");
        assert_eq!(
            store.turns("chat", &second.id)[0].text,
            "must survive contention"
        );
        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A turn taken *from a view* and handed back to be committed arrives still flagged. Recording it
    /// that way is a contradiction that never settles: `pending` means "not final", so every later
    /// read would splice a second pending answer in beside the settled one. Both flags are cleared
    /// here, and an undated turn is dated, so a stored turn always says when it landed.
    #[test]
    fn a_committed_turn_is_never_pending_or_queued_and_is_always_dated() {
        let store = scratch("flags");
        let s = store
            .create("chat", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");

        store
            .append_turn(
                "chat",
                &s.id,
                Turn {
                    at: 0,
                    pending: true,
                    queued: true,
                    ..user_turn("taken from a view")
                },
            )
            .expect("append");

        let turns = store.turns("chat", &s.id);
        assert_eq!(turns.len(), 1);
        assert!(!turns[0].pending, "a recorded turn is settled");
        assert!(!turns[0].queued, "and it has been asked");
        assert!(turns[0].at > 0, "and it says when it landed");
        assert_eq!(
            store.get("chat", &s.id).expect("listed").last_activity,
            turns[0].at,
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A page is a window, and every turn in it keeps the number it has in the whole conversation —
    /// which is the difference between a reader that can address turn 83 and one that thinks it is
    /// turn 3. The count beside it is of the conversation, not of the page.
    #[test]
    fn a_page_numbers_its_turns_by_where_they_are_in_the_conversation() {
        let store = scratch("page");
        let s = store
            .create("chat", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");
        for i in 0..10 {
            store
                .append_turn("chat", &s.id, user_turn(format!("q{i}")))
                .expect("append");
        }

        let newest = store.transcript_page("chat", &s.id, None, 3, None, false);
        assert_eq!(newest.total, 10, "what the page is a window onto");
        assert_eq!(newest.recorded, 10);
        assert_eq!(
            newest
                .turns
                .iter()
                .map(|(seq, t)| (*seq, t.text.as_str()))
                .collect::<Vec<_>>(),
            [(7, "q7"), (8, "q8"), (9, "q9")],
            "the newest three, oldest first, each with its own place",
        );

        let earlier = store.transcript_page("chat", &s.id, Some(7), 3, None, false);
        assert_eq!(
            earlier
                .turns
                .iter()
                .map(|(seq, _)| *seq)
                .collect::<Vec<_>>(),
            [4, 5, 6],
            "`before` is exclusive, so this is the page above the one already read",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The answer being written belongs at the end of the conversation, so it rides the newest page
    /// and no other. A reader paging backwards through history would otherwise meet the live answer
    /// again at the foot of every page they opened.
    #[test]
    fn the_answer_in_flight_rides_the_newest_page_only() {
        let store = scratch("page-live");
        let s = store
            .create("chat", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");
        for i in 0..4 {
            store
                .append_turn("chat", &s.id, user_turn(format!("q{i}")))
                .expect("append");
        }
        let live = || {
            Some(TurnContent {
                text: "working".to_string(),
                steps: Vec::new(),
                metrics: None,
            })
        };

        let newest = store.transcript_page("chat", &s.id, None, 2, live(), true);
        assert_eq!(newest.recorded, 4, "four rows…");
        assert_eq!(
            newest.total, 5,
            "…and the answer being written is a turn to a reader"
        );
        let (seq, last) = newest.turns.last().expect("a turn");
        assert!(last.pending, "spliced in behind the newest question");
        assert_eq!(
            *seq, 4,
            "numbered from the end of the conversation, not of the page"
        );

        let earlier = store.transcript_page("chat", &s.id, Some(4), 2, live(), true);
        assert!(
            earlier.turns.iter().all(|(_, t)| !t.pending),
            "and never onto a page that is not the newest",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// One turn that will not decode must not take the conversation with it. The tolerance moved
    /// from a torn last line to an unreadable row, but the rule is the same one.
    #[test]
    fn a_row_that_will_not_decode_is_skipped_rather_than_failing_the_read() {
        let store = scratch("torn");
        let s = store
            .create("chat", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");
        store
            .append_turn("chat", &s.id, user_turn("first"))
            .expect("append");
        store
            .append_turn("chat", &s.id, user_turn("third"))
            .expect("append");

        // Something wrote a turn this build cannot read — a torn write, or a newer shape.
        super::super::db::conn(&store.db_path())
            .expect("conn")
            .execute(
                "INSERT INTO turns (agent, session, seq, at, role, json)
                 VALUES ('chat', ?1, 99, 1, 'user', '{ not json')",
                [&s.id],
            )
            .expect("insert");

        let turns = store.turns("chat", &s.id);
        assert_eq!(
            turns.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["first", "third"],
            "the readable turns survive, in order",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A call the engine never answered, which is how an interrupted run almost always ends: the
    /// child was killed between writing the invocation and reading the result.
    fn open_call() -> Step {
        Step::Tool {
            name: "Edit".to_string(),
            input: "{}".to_string(),
            status: ToolStatus::Running,
            output: String::new(),
        }
    }

    fn statuses(turn: &Turn) -> Vec<ToolStatus> {
        turn.steps
            .iter()
            .filter_map(|s| match s {
                Step::Tool { status, .. } => Some(*status),
                _ => None,
            })
            .collect()
    }

    /// A recorded turn is final, so nothing in it can still be running — the same contradiction as a
    /// recorded `pending` flag, and it does not settle either. Left alone, an interrupted run kept a
    /// green "running" call at the top of a conversation that had been over for weeks.
    #[test]
    fn a_recorded_turn_never_keeps_a_call_open() {
        let store = scratch("open-call");
        let s = store
            .create("chat", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");

        store
            .append_turn(
                "chat",
                &s.id,
                assistant_turn(&TurnContent {
                    text: String::new(),
                    steps: vec![open_call()],
                    metrics: None,
                }),
            )
            .expect("append");

        assert_eq!(
            statuses(&store.turns("chat", &s.id)[0]),
            [ToolStatus::Unanswered],
            "a committed call that never came back is unanswered, not running",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// And the rows written before that was true are still on disk. A conversation from last week
    /// does not get a second chance to be recorded, so the read is what heals it.
    #[test]
    fn a_row_recorded_with_an_open_call_is_closed_on_the_way_out() {
        let store = scratch("heal");
        let s = store
            .create("chat", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");

        // Written the way an older build wrote it: committed, and still claiming to be running.
        let json = serde_json::to_string(&Turn {
            steps: vec![open_call()],
            ..assistant_turn(&TurnContent::default())
        })
        .expect("encode");
        super::super::db::conn(&store.db_path())
            .expect("conn")
            .execute(
                "INSERT INTO turns (agent, session, seq, at, role, json)
                 VALUES ('chat', ?1, 0, 1, 'assistant', ?2)",
                rusqlite::params![&s.id, json],
            )
            .expect("insert");

        assert_eq!(
            statuses(&store.turns("chat", &s.id)[0]),
            [ToolStatus::Unanswered],
            "an old row reads as what it is, however it was written",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The answer being written right now is the one place a call may still be open — and only for
    /// as long as there is a child left to answer it. `running` decides, because it is the only
    /// thing here that knows.
    #[test]
    fn the_spliced_answer_keeps_its_calls_open_only_while_the_child_is_alive() {
        let live = || TurnContent {
            text: "working".to_string(),
            steps: vec![open_call()],
            metrics: None,
        };
        let asked = vec![user_turn("go")];

        let alive = view(asked.clone(), Some(live()), true, Vec::new());
        assert_eq!(
            statuses(&alive[1]),
            [ToolStatus::Running],
            "a child still working can still answer its call",
        );

        let exited = view(asked, Some(live()), false, Vec::new());
        assert_eq!(
            statuses(&exited[1]),
            [ToolStatus::Unanswered],
            "and one that has exited never will",
        );
    }

    fn event(name: &str) -> Step {
        Step::Event {
            name: name.to_string(),
            event_type: "agent.started".to_string(),
            metadata: serde_json::Value::Null,
        }
    }

    /// A step kind from a newer build decodes as `Unknown` instead of taking its whole turn with
    /// it — what every reader older than `Step::Event` did to an answer holding one.
    #[test]
    fn regression_unknown_step_kind_keeps_its_turn() {
        let json =
            r#"{"role":"assistant","text":"kept","steps":[{"kind":"from_the_future","x":1}]}"#;
        let turn: Turn =
            serde_json::from_str(json).expect("an unknown step must not fail the turn");
        assert_eq!(turn.text, "kept");
        assert_eq!(turn.steps, vec![Step::Unknown]);
    }

    /// Events held for an answer are folded into whichever path records that answer, and their
    /// rows go with it — not only the settle path, and never left for a `seq` already taken.
    #[test]
    fn held_events_fold_into_the_recorded_answer_and_are_deleted() {
        let store = scratch("held-events");
        let record = store
            .create("chat", Backend::HarnessAdi, "/tmp", "q")
            .unwrap();
        store
            .append_turn("chat", &record.id, user_turn("q"))
            .unwrap();
        store
            .record_event("chat", &record.id, 1, event("first"))
            .unwrap();
        store
            .record_event("chat", &record.id, 0, event("zeroth"))
            .unwrap();
        assert_eq!(store.live_events("chat", &record.id).len(), 2);

        let mut answer = assistant_turn(&TurnContent::default());
        answer.steps = vec![Step::Thinking { text: "t".into() }];
        assert!(store.answer("chat", &record.id, answer.clone()).unwrap());

        let turns = store.turns("chat", &record.id);
        assert_eq!(
            turns[1].steps,
            vec![
                event("zeroth"),
                Step::Thinking { text: "t".into() },
                event("first")
            ]
        );
        assert!(store.live_events("chat", &record.id).is_empty());
        let conn = super::super::db::conn(&store.db_path()).unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 0, "settled events must not stay behind");

        assert!(
            !store.answer("chat", &record.id, answer).unwrap(),
            "a second settle of the same question is a no-op"
        );
        store
            .record_event("chat", &record.id, 0, event("late"))
            .unwrap();
        let turns = store.turns("chat", &record.id);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].steps.last(), Some(&event("late")));
    }
}
