//! What a session said: the `turns` table, one row per turn, oldest first by `seq`.
//!
//! A runner never sees this. It answers one turn at a time and knows nothing about the ones before
//! it; what the session *is* — the sequence of questions and answers a reader scrolls through — is
//! durable, so it belongs here with the record and the queue.
//!
//! Append-only. A turn that has landed is final, and two writers appending never collide because
//! the sequence number is taken inside the same transaction as the insert.
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

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::progress::{Step, TurnContent, TurnMetrics};

use super::db::sql_err;

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
    /// The assistant turn's activity — tool calls and thinking — parsed from the engine's output.
    /// Empty for user turns and for engines that emit no structured progress.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// The assistant turn's telemetry (tokens / cost / duration), when the engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TurnMetrics>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// The two roles a turn carries. Strings rather than an enum because [`Turn::role`] is one, and it
/// crosses the wire to readers that already expect these exact words.
pub(crate) const ROLE_USER: &str = "user";
pub(crate) const ROLE_ASSISTANT: &str = "assistant";

/// A question, ready to append. What the store records when a message is actually put to the agent —
/// not when it was typed, which is what the queue is for.
#[must_use]
pub fn user_turn(text: impl Into<String>) -> Turn {
    Turn {
        role: ROLE_USER.to_string(),
        text: text.into(),
        at: now_ms(),
        pending: false,
        queued: false,
        steps: Vec::new(),
        metrics: None,
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
        steps: content.steps.clone(),
        metrics: content.metrics.clone(),
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
/// All of it in one transaction: the sequence number is read and used under the same lock, so two
/// processes appending at once cannot mint the same one, and a session's activity can never
/// disagree with its last turn.
///
/// # Errors
/// Returns [`Error::NotFound`] when the session is gone, and database errors.
pub(super) fn append(conn: &Connection, agent: &str, id: &str, mut turn: Turn) -> Result<()> {
    turn.pending = false;
    turn.queued = false;
    if turn.at == 0 {
        turn.at = now_ms();
    }
    let json = serde_json::to_string(&turn)
        .map_err(|e| Error::Session(format!("couldn't encode a turn of session {id}: {e}")))?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("record a turn in", e))?;
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
    tx.commit().map_err(|e| sql_err("record a turn in", e))?;
    Ok(())
}

/// The recorded turns, oldest first.
///
/// A row that will not decode is skipped rather than failing the read — one torn turn must not take
/// a whole conversation out of the view.
pub(super) fn load(conn: &Connection, agent: &str, id: &str) -> Vec<Turn> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT json FROM turns WHERE agent = ?1 AND session = ?2 ORDER BY seq",
    ) else {
        return Vec::new();
    };
    stmt.query_map([agent, id], |row| row.get::<_, String>(0))
        .map(|rows| {
            rows.flatten()
                .filter_map(|json| serde_json::from_str::<Turn>(&json).ok())
                .collect()
        })
        .unwrap_or_default()
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
/// streaming any more.
pub(super) fn view(
    mut turns: Vec<Turn>,
    live: Option<TurnContent>,
    running: bool,
    queued: Vec<String>,
) -> Vec<Turn> {
    if let Some(content) = live
        && turns.last().map(|t| t.role.as_str()) == Some(ROLE_USER)
    {
        turns.push(Turn {
            role: ROLE_ASSISTANT.to_string(),
            text: content.text,
            // No recorded moment: nothing has been recorded. It gets one when it is appended.
            at: 0,
            pending: running,
            queued: false,
            steps: content.steps,
            metrics: content.metrics,
        });
    }
    turns.extend(queued.into_iter().map(|text| Turn {
        role: ROLE_USER.to_string(),
        text,
        at: 0,
        pending: false,
        queued: true,
        steps: Vec::new(),
        metrics: None,
    }));
    turns
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionStore;
    use crate::Backend;

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
        // The moment is also what the session now reads as active at.
        assert_eq!(
            store.get("chat", &s.id).expect("listed").last_activity,
            turns[0].at,
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
}
