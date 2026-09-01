//! What a conversation is *for*: the `goals` table, one row per goal.
//!
//! A turn ends when the model stops calling tools, and nothing about the turn says whether the work
//! it was doing is finished — only that this particular burst of it is. A **goal** is the answer to
//! that: a sentence saying what "done" means, kept beside the conversation, and re-put to the run
//! every time it falls quiet. The run answers by closing the goal, one way or the other.
//!
//! # Two verbs close a goal, and the platform is neither of them
//!
//! [`GoalState::Met`] and [`GoalState::GivenUp`] are the only exits, and both are written by
//! somebody calling the CLI — the run itself, or a person. Nothing here counts attempts, and no
//! sweep decides a goal has gone on long enough: a run that cannot finish says so with
//! `knowingly-give-up`, which is a sentence somebody has to write and is worth reading afterward.
//! The platform re-asks; it never answers.
//!
//! # Closing is a conditional UPDATE, and a second call is a no-op rather than an error
//!
//! Same shape as [`questions::resolve`](super::questions::resolve) and for half the same reason —
//! the app, the CLI and a run's own child all reach this store, so exactly one caller may write the
//! ending. The other half is a rule this table does not share with any other: **a call on this path
//! is never refused**. Closing a goal that is already closed changes nothing and says so; it does
//! not fail. A run reading a failed tool result would try again, or worse, argue with it.
//!
//! # A goal id is unique across the store
//!
//! `g-<millis>-<seq>`, like an ask's and an await's — but goals are looked up by id *alone*, not by
//! `(agent, session, id)`. What quotes the id back is a nudge the run reads, and what types it is
//! often the run's own shell: making the id sufficient means `goals met g-…` works without the
//! caller having to also know which conversation it is standing in.

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

use super::db::{now_ms, sql_err};

/// The longest a goal may be. A goal is re-read by the model on every nudge, so it is charged for
/// again and again — and one that does not fit here is a plan, which belongs in the conversation.
const MAX_TEXT: usize = 2_000;

/// Where a goal is in its life. `Open` is the only state that is nudged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    /// Still being worked toward — re-put to the conversation whenever it falls quiet.
    Open,
    /// Done, by the judgement of whoever ran `goals met`.
    Met,
    /// Not done, and not going to be. Written by `goals knowingly-give-up`, which is deliberately a
    /// mouthful: it is an admission, and the record keeps the reason.
    GivenUp,
}

impl GoalState {
    /// The wire and column spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Met => "met",
            Self::GivenUp => "given_up",
        }
    }

    /// Back from the column. An unreadable value reads as `Open` — a goal whose state was written by
    /// a newer build is still a goal somebody set, and dropping it would silently stop the nudges
    /// that are the whole point of the row.
    fn from_str(text: &str) -> Self {
        match text {
            "met" => Self::Met,
            "given_up" => Self::GivenUp,
            _ => Self::Open,
        }
    }
}

/// Who set the goal. Not bookkeeping: a goal a run wrote for itself is a run that has decided what
/// it is doing, and reading the two apart afterward is how you tell a plan that came from a person
/// from one the machine talked itself into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetBy {
    Human,
    Agent,
}

impl SetBy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    fn from_str(text: &str) -> Self {
        match text {
            "agent" => Self::Agent,
            _ => Self::Human,
        }
    }
}

/// One goal, open or closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    /// Unique across the store — see the module note on why it is not scoped to its conversation.
    pub id: String,
    pub agent: String,
    /// The conversation this goal is nudged into.
    pub conv: String,
    /// What done means, in whoever-set-it's own words.
    pub text: String,
    pub state: GoalState,
    pub set_by: SetBy,
    /// Unix milliseconds.
    pub created_at: u64,
    /// When this goal was last put to the conversation; 0 until the first nudge. The floor between
    /// nudges is measured from here.
    pub last_nudge_at: u64,
    /// How many times it has been put. Reported, never enforced.
    pub nudges: u64,
    pub closed_at: Option<u64>,
    /// The evidence a `met` carried, or the reason a give-up did. Empty when the goal is open.
    #[serde(default)]
    pub note: String,
}

impl Goal {
    #[must_use]
    pub fn open(&self) -> bool {
        self.state == GoalState::Open
    }

    /// The one-line version, for a rail badge or a listing.
    #[must_use]
    pub fn headline(&self) -> String {
        let text = self.text.trim();
        let mut line: String = text.chars().take(72).collect();
        if text.chars().count() > 72 {
            line.push('…');
        }
        line
    }
}

/// Write down a goal and return it.
///
/// Nothing about an existing goal is consulted: a conversation may carry several at once, and one
/// set while another is open is somebody adding to the list rather than contradicting it. The nudge
/// carries all of them.
///
/// # Errors
/// [`Error::Arguments`] for empty or oversized text — the only two things that make a goal
/// unusable rather than merely ambitious. Database errors otherwise.
pub(super) fn create(
    conn: &Connection,
    agent: &str,
    conv: &str,
    text: &str,
    set_by: SetBy,
) -> Result<Goal> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Arguments(
            "a goal needs text — say what would make this conversation done".to_string(),
        ));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(Error::Arguments(format!(
            "that goal is longer than the {MAX_TEXT} characters a goal may carry — it is re-read \
             every time you are asked about it, so say what done means and leave the plan to the \
             conversation"
        )));
    }
    // Asked before the insert only so the answer is readable. The foreign key would refuse this
    // anyway, but as "FOREIGN KEY constraint failed" — and the caller is often a model reading it
    // as a tool result, which is exactly the audience least able to act on that.
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM sessions WHERE agent = ?1 AND id = ?2",
            [agent, conv],
            |_| Ok(true),
        )
        .optional()
        .map_err(|e| sql_err("record a goal in", e))?
        .unwrap_or(false);
    if !exists {
        return Err(Error::NotFound(format!(
            "{agent} has no conversation {conv} to set a goal on"
        )));
    }

    let now = now_ms();
    let goal = Goal {
        id: new_id(now),
        agent: agent.to_string(),
        conv: conv.to_string(),
        text: text.to_string(),
        state: GoalState::Open,
        set_by,
        created_at: now,
        last_nudge_at: 0,
        nudges: 0,
        closed_at: None,
        note: String::new(),
    };
    conn.execute(
        "INSERT INTO goals
           (agent, session, id, text, state, set_by, created_at, last_nudge_at, nudges, closed_at, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, NULL, '')",
        rusqlite::params![
            agent,
            conv,
            goal.id,
            goal.text,
            goal.state.as_str(),
            goal.set_by.as_str(),
            goal.created_at,
        ],
    )
    .map_err(|e| sql_err("record a goal in", e))?;
    Ok(goal)
}

/// Reword a goal, returning it as it now reads — or `None` when there is no such goal.
///
/// An open goal only. Rewording one that is already closed would rewrite what somebody met or gave
/// up on, and the closed row is the record of that decision.
///
/// # Errors
/// [`Error::Arguments`] for empty or oversized text; database errors otherwise.
pub(super) fn edit(conn: &Connection, id: &str, text: &str) -> Result<Option<Goal>> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Arguments(
            "a goal needs text — say what would make this conversation done".to_string(),
        ));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(Error::Arguments(format!(
            "that goal is longer than the {MAX_TEXT} characters a goal may carry"
        )));
    }
    conn.execute(
        "UPDATE goals SET text = ?2 WHERE id = ?1 AND state = 'open'",
        rusqlite::params![id, text],
    )
    .map_err(|e| sql_err("reword a goal in", e))?;
    by_id(conn, id)
}

/// What [`close_goal`](super::SessionStore::close_goal) did, so the caller can say so without
/// asking again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// This call is what closed it.
    Now(Goal),
    /// It was closed already — by the other verb, by a person, or by this same call arriving twice.
    /// Not an error: see the module note.
    Already(Goal),
    /// No goal has that id.
    Unknown,
}

/// Close a goal as met or given up, with the evidence or the reason.
///
/// Never fails for being late. A goal closed twice, or met after it was given up, answers
/// [`Closed::Already`] with the row as it stands — the first ending is the one that happened, and
/// the second caller is told which without being refused.
///
/// # Errors
/// Returns database errors only.
pub(super) fn close(conn: &Connection, id: &str, state: GoalState, note: &str) -> Result<Closed> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("close a goal in", e))?;
    let Some(before) = by_id(&tx, id)? else {
        return Ok(Closed::Unknown);
    };
    // The row is the lock, held by the database rather than by any one process — the app, the CLI
    // and the run's own child all reach this, and only one of them may write the ending.
    let changed = tx
        .execute(
            "UPDATE goals SET state = ?2, note = ?3, closed_at = ?4
             WHERE id = ?1 AND state = 'open'",
            rusqlite::params![id, state.as_str(), note.trim(), now_ms()],
        )
        .map_err(|e| sql_err("close a goal in", e))?;
    if changed == 0 {
        return Ok(Closed::Already(before));
    }
    let after = by_id(&tx, id)?.unwrap_or(before);
    tx.commit().map_err(|e| sql_err("close a goal in", e))?;
    Ok(Closed::Now(after))
}

/// Stamp every open goal of a conversation as just nudged, and count the nudge.
///
/// Per conversation rather than per goal because a nudge *is* per conversation: all of its open
/// goals travel in one message, so they were all put at the same moment and the floor before the
/// next one is shared.
///
/// # Errors
/// Returns database errors.
pub(super) fn mark_nudged(conn: &Connection, agent: &str, conv: &str, at: u64) -> Result<()> {
    conn.execute(
        "UPDATE goals SET last_nudge_at = ?3, nudges = nudges + 1
         WHERE agent = ?1 AND session = ?2 AND state = 'open'",
        rusqlite::params![agent, conv, at],
    )
    .map_err(|e| sql_err("record a nudge in", e))?;
    Ok(())
}

/// One goal by id, wherever it lives.
pub(super) fn by_id(conn: &Connection, id: &str) -> Result<Option<Goal>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM goals WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()
    .map_err(|e| sql_err("read a goal from", e))
}

/// Every goal of one conversation, oldest first — open and closed alike, because what was already
/// met is the context for what is being asked now.
pub(super) fn for_conversation(conn: &Connection, agent: &str, conv: &str) -> Vec<Goal> {
    let Ok(mut stmt) = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM goals WHERE agent = ?1 AND session = ?2 ORDER BY created_at, id"
    )) else {
        return Vec::new();
    };
    stmt.query_map([agent, conv], from_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Every open goal anywhere, oldest first.
///
/// One query for the whole store, for the same reason the ask inbox is one query: the caller is a
/// sweep that runs every second and finds nothing in nearly every pass, so the *empty* answer is
/// the one that has to be cheap. The partial index is what makes it a lookup rather than a scan of
/// every goal ever set.
pub(super) fn all_open(conn: &Connection) -> Vec<Goal> {
    let Ok(mut stmt) = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM goals WHERE state = 'open' ORDER BY created_at, id"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], from_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Drop every *open* goal of an agent, returning how many went — what deleting the agent takes.
///
/// Only the open ones, exactly as with questions: a closed goal is history that belongs with the
/// transcript explaining it, and an open one is a thing to *do* in a conversation nothing can nudge
/// any more.
pub(super) fn forget_agent(conn: &Connection, agent: &str) -> usize {
    conn.execute(
        "DELETE FROM goals WHERE agent = ?1 AND state = 'open'",
        [agent],
    )
    .unwrap_or(0)
}

// ---- internals ---------------------------------------------------------------------

/// The columns [`from_row`] reads, in its order.
const COLUMNS: &str = "id, agent, session, text, state, set_by, created_at, last_nudge_at, \
                       nudges, closed_at, note";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Goal> {
    let state: String = row.get(4)?;
    let set_by: String = row.get(5)?;
    Ok(Goal {
        id: row.get(0)?,
        agent: row.get(1)?,
        conv: row.get(2)?,
        text: row.get(3)?,
        state: GoalState::from_str(&state),
        set_by: SetBy::from_str(&set_by),
        created_at: row.get(6)?,
        last_nudge_at: row.get(7)?,
        nudges: row.get(8)?,
        closed_at: row.get(9)?,
        note: row.get(10)?,
    })
}

/// A unique, time-sortable goal id. Short and obviously an id, because it is quoted back to the
/// model in every nudge and typed by hand at a terminal.
fn new_id(now: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("g-{now:013}-{seq:04}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;
    use crate::store::SessionStore;

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-goals-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        super::super::db::forget_connections();
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    fn open_session(store: &SessionStore, agent: &str) -> String {
        store
            .create(agent, Backend::HarnessAdi, "/tmp", "go")
            .expect("create")
            .id
    }

    /// The round trip: a goal outlives the turn that set it, so it has to come back as written.
    #[test]
    fn a_goal_is_open_until_one_of_the_two_verbs_closes_it() {
        let store = scratch("round-trip");
        let conv = open_session(&store, "chat");

        let goal = store
            .create_goal("chat", &conv, "the tests pass", SetBy::Human)
            .expect("create");
        assert!(goal.open());
        assert_eq!(store.open_goals("chat", &conv).len(), 1);

        let closed = store
            .close_goal(&goal.id, GoalState::Met, "cargo test: 185 passed")
            .expect("close");
        let Closed::Now(met) = closed else {
            panic!("this call is what closed it");
        };
        assert_eq!(met.state, GoalState::Met);
        assert_eq!(met.note, "cargo test: 185 passed");
        assert!(met.closed_at.is_some());
        assert!(
            store.open_goals("chat", &conv).is_empty(),
            "a closed goal stops being nudged"
        );
        assert_eq!(store.goals("chat", &conv).len(), 1, "and stays as history");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The rule the whole CLI surface rests on: this path never refuses. Closing twice, or closing
    /// with the other verb after the fact, reports what already happened instead of erroring — a
    /// run that reads a failed tool result tries again, or argues with it.
    #[test]
    fn closing_a_closed_goal_is_a_no_op_and_never_an_error() {
        let store = scratch("never-blocks");
        let conv = open_session(&store, "chat");
        let goal = store
            .create_goal("chat", &conv, "ship it", SetBy::Agent)
            .expect("create");

        store
            .close_goal(&goal.id, GoalState::Met, "done")
            .expect("first close");
        let second = store
            .close_goal(&goal.id, GoalState::GivenUp, "actually no")
            .expect("a second close is not an error");

        let Closed::Already(still) = second else {
            panic!("the first ending is the one that happened");
        };
        assert_eq!(still.state, GoalState::Met);
        assert_eq!(still.note, "done", "and its evidence is not overwritten");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// An id nobody minted is answered, not refused — a typo at a terminal must not read as a
    /// failure the model then works around.
    #[test]
    fn an_unknown_goal_id_is_answered_rather_than_refused() {
        let store = scratch("unknown");
        assert_eq!(
            store
                .close_goal("g-nope", GoalState::Met, "")
                .expect("not an error"),
            Closed::Unknown
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Setting a goal on a conversation that is not there is the one create-time failure, and it
    /// has to read as one: the foreign key would catch it regardless, but "FOREIGN KEY constraint
    /// failed" is unreadable to the model most likely to hit it.
    #[test]
    fn a_goal_on_a_conversation_that_does_not_exist_says_so_plainly() {
        let store = scratch("no-session");
        let err = store
            .create_goal("ghost", "1700000000000-0001", "finish", SetBy::Agent)
            .expect_err("refused");
        assert!(err.to_string().contains("no conversation"), "{err}");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A conversation may carry several goals, and a nudge puts all of them at once — so the floor
    /// before the next nudge is stamped across the whole conversation, not per goal.
    #[test]
    fn a_nudge_stamps_every_open_goal_of_its_conversation() {
        let store = scratch("nudge");
        let conv = open_session(&store, "chat");
        store
            .create_goal("chat", &conv, "first", SetBy::Human)
            .expect("create");
        let second = store
            .create_goal("chat", &conv, "second", SetBy::Agent)
            .expect("create");
        store
            .close_goal(&second.id, GoalState::Met, "")
            .expect("close");
        let third = store
            .create_goal("chat", &conv, "third", SetBy::Agent)
            .expect("create");

        store
            .mark_goals_nudged("chat", &conv, 1_700)
            .expect("nudge");

        let open = store.open_goals("chat", &conv);
        assert_eq!(open.len(), 2);
        assert!(
            open.iter()
                .all(|g| g.last_nudge_at == 1_700 && g.nudges == 1)
        );
        assert_eq!(
            store
                .goal(&second.id)
                .expect("read")
                .expect("still there")
                .nudges,
            0,
            "a closed goal is not nudged and does not count one"
        );
        assert_eq!(
            store.goal(&third.id).expect("read").expect("there").nudges,
            1
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The sweep's one query. It runs every second and finds nothing in nearly every pass, so what
    /// it must get right is the empty answer and the cross-agent one.
    #[test]
    fn the_sweep_sees_every_open_goal_in_the_store() {
        let store = scratch("all-open");
        let one = open_session(&store, "chat");
        let two = open_session(&store, "other");
        assert!(store.all_open_goals().is_empty());

        store
            .create_goal("chat", &one, "a", SetBy::Human)
            .expect("create");
        let closed = store
            .create_goal("other", &two, "b", SetBy::Human)
            .expect("create");
        store
            .create_goal("other", &two, "c", SetBy::Agent)
            .expect("create");
        store
            .close_goal(&closed.id, GoalState::GivenUp, "cannot reach the host")
            .expect("close");

        let open = store.all_open_goals();
        assert_eq!(open.len(), 2);
        assert!(open.iter().all(Goal::open));

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Rewording is for a goal still being worked toward. A closed one is the record of a decision.
    #[test]
    fn editing_leaves_a_closed_goal_as_it_was_decided() {
        let store = scratch("edit");
        let conv = open_session(&store, "chat");
        let goal = store
            .create_goal("chat", &conv, "vague", SetBy::Human)
            .expect("create");

        let reworded = store
            .edit_goal(&goal.id, "specific: the suite is green")
            .expect("edit")
            .expect("found");
        assert_eq!(reworded.text, "specific: the suite is green");

        store
            .close_goal(&goal.id, GoalState::Met, "green")
            .expect("close");
        let after = store
            .edit_goal(&goal.id, "sneaking a change in")
            .expect("edit")
            .expect("found");
        assert_eq!(after.text, "specific: the suite is green");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Deleting a conversation takes its goals, so nothing nudges a chat that is gone.
    #[test]
    fn deleting_a_session_takes_its_goals() {
        let store = scratch("cascade");
        let conv = open_session(&store, "chat");
        store
            .create_goal("chat", &conv, "still there?", SetBy::Human)
            .expect("create");
        assert_eq!(store.all_open_goals().len(), 1);

        store.delete("chat", &conv).expect("delete");
        assert!(store.all_open_goals().is_empty());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Deleting the agent takes what nothing can nudge any more, and leaves the history.
    #[test]
    fn deleting_an_agent_forgets_only_what_is_still_open() {
        let store = scratch("forget-agent");
        let conv = open_session(&store, "chat");
        let met = store
            .create_goal("chat", &conv, "done already", SetBy::Human)
            .expect("create");
        store
            .close_goal(&met.id, GoalState::Met, "")
            .expect("close");
        store
            .create_goal("chat", &conv, "still open", SetBy::Human)
            .expect("create");

        assert_eq!(store.forget_goals("chat"), 1);
        assert!(store.all_open_goals().is_empty());
        assert_eq!(store.goals("chat", &conv).len(), 1);

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
