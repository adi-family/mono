//! The session store: everything durable about a session that is *not* the process running it.
//!
//! A runner is a process manager, so it keeps nothing. The record, the queue, the hidden flag, the
//! log, and the runner's own opaque state slot all live here — see `docs/agent-runner.md` for the
//! layering this is one third of.
//!
//! # A database, and one file per session
//!
//! ```text
//! <sessions_dir>/sessions.db              every session, every turn, every queued message
//! <sessions_dir>/<agent>/<id>.log         the raw output a runner spools into
//! <sessions_dir>/<agent>/<id>.<whatever>  sidecars a runner invents
//! ```
//!
//! The log stays a file because it has to: a spawned child needs a real file descriptor to redirect
//! stdout and stderr into, and that is not a thing a row can be. Everything else — the record, the
//! transcript, the queue — is a row, because everything else is *listed*, and listing them as files
//! meant a `read_dir` per agent plus an open and a `stat` per session on every poll. See
//! [`db`](self::db) for the profile that settled it.
//!
//! A session still owns the whole `<id>.*` namespace of its agent directory, which is why deleting
//! sweeps by prefix: the store cannot know what a runner parked beside the log.
//!
//! # Backend-agnostic, which is a property of the *key*
//!
//! A session is found by the two things that never change — who it belongs to, and what it is
//! called — and its backend is a column. It used to be a directory level
//! (`<sessions>/<process|harness>/<agent>/…`), so a stored field decided where history was read
//! from: change an agent's `backend` and every run it had ever done vanished, unlistable and
//! undeletable, because the listing looked under the new executor's directory while the files sat
//! under the old one's. As a column it is a label on a row that stays exactly where it was filed.

mod attachments;
mod db;
mod goals;
mod queue;
mod questions;
mod record;
mod session;
mod transcript;

use std::path::{Path, PathBuf};

use crate::Backend;
use crate::error::{Error, Result};
use crate::progress::TurnContent;

pub use attachments::{Attachment, MAX_BYTES as MAX_ATTACHMENT_BYTES, MEDIA_TYPES, is_supported};
pub use db::now_ms;
pub use goals::{Closed as GoalClosed, Goal, GoalState, SetBy};
pub use queue::QueuedMessage;
pub use questions::{
    Answer, AnsweredBy, Ask, Choice, MAX_QUESTIONS, Question, Request as AskRequest,
};
pub use record::{RunOutcome, SessionRecord};
pub use session::SessionRef;
pub use transcript::{Turn, assistant_turn, user_turn, user_turn_with};

/// The role a question carries — what the agent layer reads to tell an unanswered turn from a
/// settled one before it commits an answer behind it.
pub(crate) use transcript::ROLE_USER;

/// How many sessions to keep per agent before [`prune_old`](SessionStore::prune_old) sweeps the
/// oldest finished ones.
pub const MAX_SESSIONS: usize = 50;

/// The database's name inside the store's root.
const DB_FILE: &str = "sessions.db";

/// The columns of a record, in the order [`record::from_row`] reads them.
const RECORD_COLUMNS: &str = "agent, id, backend, cwd, message, started_at, last_activity, \
                              hidden, runner_state, outcome, runner";

/// The sessions under one root.
///
/// Cheap to clone and safe to hold: it owns a path and no cached state, so two of them over the
/// same root are the same store and cannot disagree. That is not tidiness — the CLI, the app, and
/// every trigger's child open this independently, so anything cached here would be a second truth
/// one of the others has already invalidated.
#[derive(Debug, Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    #[must_use]
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: sessions_dir.into(),
        }
    }

    /// The root every agent's sessions live under.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where one agent's files are — the log, and whatever a runner leaves beside it.
    #[must_use]
    pub fn agent_dir(&self, agent: &str) -> PathBuf {
        self.dir.join(agent)
    }

    /// Where the database lives.
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.dir.join(DB_FILE)
    }

    /// This thread's connection, opened on first use.
    fn conn(&self) -> Result<std::rc::Rc<rusqlite::Connection>> {
        db::conn(&self.db_path())
    }

    /// Where a session's raw output goes.
    ///
    /// A real path rather than a sink, because a spawned child needs a file descriptor to redirect
    /// stdout and stderr into. Answered for any id, existing or not: this is the address a runner is
    /// about to create, not a claim that it is there.
    #[must_use]
    pub fn log_path(&self, agent: &str, id: &str) -> PathBuf {
        record::log_path(&self.agent_dir(agent), id)
    }

    /// A borrowed [`Session`](crate::runner::Session) view of one session, for handing to a runner.
    ///
    /// Every question it answers is a fresh read — which is what a runner about to *write* needs,
    /// and why this is the default. For the read-only sweep a listing performs, see
    /// [`session_as_listed`](Self::session_as_listed).
    #[must_use]
    pub fn session(&self, agent: &str, id: &str) -> SessionRef<'_> {
        SessionRef::new(self, agent, id)
    }

    /// The same view, answering [`Session::state`](crate::runner::Session::state) from the record it
    /// was listed with instead of asking again.
    ///
    /// A listing reads every session's row and then asks its runner whether it is alive, which reads
    /// the runner's state slot — a column the row already carried. Re-querying it made the sweep two
    /// reads per session for one instant's answer, and a listing *is* one instant: the fresher value
    /// would only be a different snapshot of the same race.
    ///
    /// **For reads only.** A runner that goes on to write must take [`session`](Self::session), or
    /// it will decide what to write from a copy another process has already moved past.
    #[must_use]
    pub fn session_as_listed<'a>(&'a self, record: &SessionRecord) -> SessionRef<'a> {
        SessionRef::as_listed(self, &record.agent, &record.id, record.runner_state.clone())
    }

    // ---- records -------------------------------------------------------------------

    /// Open a session for `agent` and return its record.
    ///
    /// `cwd` is pinned here and re-used for every later turn — an engine's session store is keyed by
    /// the directory it ran in, so a session that moves is a session that cannot be resumed.
    ///
    /// No log is written: the log appearing is what says a turn has started
    /// ([`Session::has_started`](crate::runner::Session::has_started)), and creating an empty one
    /// here would tell every runner it was resuming a session nothing had ever run in.
    ///
    /// Nothing is pruned either. Only the caller knows what is still alive, so
    /// [`prune_old`](Self::prune_old) is its own call rather than a side effect of this one.
    ///
    /// # Errors
    /// Returns directory-creation and database errors.
    pub fn create(
        &self,
        agent: &str,
        backend: Backend,
        cwd: impl Into<PathBuf>,
        message: &str,
    ) -> Result<SessionRecord> {
        self.create_as(agent, backend, None, cwd, message)
    }

    /// [`create`](Self::create), naming the runner that is going to drive it.
    ///
    /// The backend says which engine the agent is pointed at; the runner says who is actually
    /// driving, and the two are not the same question. A simulated run has the agent's own backend
    /// — that is the point of it — and a person in the model's seat, so nothing but this
    /// distinguishes it, and a later read that resolved it by backend would hand it the engine's
    /// runner and be told the run is dead.
    ///
    /// `None` records nothing and reads back as "whatever runs its backend", which is what every
    /// session opened before this existed was.
    ///
    /// # Errors
    /// Returns directory-creation and database errors.
    pub fn create_as(
        &self,
        agent: &str,
        backend: Backend,
        runner: Option<crate::runner::RunnerKind>,
        cwd: impl Into<PathBuf>,
        message: &str,
    ) -> Result<SessionRecord> {
        // The agent's directory is made here rather than at first spawn: a runner is handed
        // `log_path` and expects to be able to open it.
        std::fs::create_dir_all(self.agent_dir(agent))?;
        let id = record::new_id();
        let started = record::started_at(&id);
        let session = SessionRecord {
            started_at: started,
            last_activity: started,
            id,
            agent: agent.to_string(),
            backend,
            runner,
            cwd: cwd.into(),
            message: message.to_string(),
            hidden: false,
            runner_state: None,
            outcome: None,
        };
        self.conn()?
            .execute(
                "INSERT INTO sessions
                   (agent, id, backend, cwd, message, started_at, last_activity, hidden, runner)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
                rusqlite::params![
                    session.agent,
                    session.id,
                    session.backend.to_string(),
                    session.cwd.to_string_lossy(),
                    session.message,
                    session.started_at,
                    session.last_activity,
                    session.runner.as_ref().map(|k| k.as_str().to_string()),
                ],
            )
            .map_err(|e| db::sql_err("open a session in", e))?;
        Ok(session)
    }

    /// One session's record, or `None` when there is nothing filed under that id.
    #[must_use]
    pub fn get(&self, agent: &str, id: &str) -> Option<SessionRecord> {
        let conn = self.conn().ok()?;
        conn.query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM sessions WHERE agent = ?1 AND id = ?2"),
            [agent, id],
            record::from_row,
        )
        .ok()
    }

    /// Every session of `agent`, newest first.
    ///
    /// By start, not by activity: the rail's own ordering is a view's business, and a listing that
    /// reshuffled itself as answers landed would be a different thing to page through. The id is the
    /// tiebreak that keeps a same-millisecond pair stable, and the pair is an index, so this is a
    /// range scan rather than a sort.
    #[must_use]
    pub fn list(&self, agent: &str) -> Vec<SessionRecord> {
        let Ok(conn) = self.conn() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare_cached(&format!(
            "SELECT {RECORD_COLUMNS} FROM sessions
             WHERE agent = ?1 ORDER BY started_at DESC, id DESC"
        )) else {
            return Vec::new();
        };
        stmt.query_map([agent], record::from_row)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Every agent that has a session filed here.
    ///
    /// What bounds the machine is the sessions that exist, not the manifests that explain them, so
    /// this deliberately includes agents whose definition has since been deleted.
    #[must_use]
    pub fn agents(&self) -> Vec<String> {
        let Ok(conn) = self.conn() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare_cached("SELECT DISTINCT agent FROM sessions") else {
            return Vec::new();
        };
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Hide (or unhide) a session: a flag on its row, so the choice outlives the tab that made it.
    /// Nothing else changes — it keeps running, keeps its log, and is still listed by everything
    /// that asks for the full history.
    ///
    /// Returns whether there was a session there to flag; an unknown id is `false`, so a stale click
    /// is idempotent rather than an error. Hiding is deliberately *not* activity — a session brought
    /// back must not have jumped to the top of the rail in the meantime.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn set_hidden(&self, agent: &str, id: &str, hidden: bool) -> Result<bool> {
        let changed = self
            .conn()?
            .execute(
                "UPDATE sessions SET hidden = ?3 WHERE agent = ?1 AND id = ?2",
                rusqlite::params![agent, id, i64::from(hidden)],
            )
            .map_err(|e| db::sql_err("hide a session in", e))?;
        Ok(changed > 0)
    }

    /// Write down how a run ended, **once**. Returns whether this call was the one that recorded it.
    ///
    /// The `outcome IS NULL` in the statement is the whole design. A run's ending is noticed by
    /// whoever happens to look first — the app's poll, a CLI listing, a trigger's child — and each
    /// of them is a separate process racing the others. Gating on the write rather than on a read
    /// beforehand makes the first one the winner and every later one a no-op, atomically, so the
    /// caller can hang "and announce it" off a `true` and know the announcement happens exactly
    /// once. A read-then-write would announce it once per watcher.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn record_outcome(&self, agent: &str, id: &str, outcome: &RunOutcome) -> Result<bool> {
        let json = serde_json::to_string(outcome).unwrap_or_else(|_| "{}".into());
        let changed = self
            .conn()?
            .execute(
                "UPDATE sessions SET outcome = ?3
                 WHERE agent = ?1 AND id = ?2 AND outcome IS NULL",
                rusqlite::params![agent, id, json],
            )
            .map_err(|e| db::sql_err("record a run's outcome in", e))?;
        Ok(changed > 0)
    }

    /// The tool section this conversation was opened with, if one was ever frozen for it.
    #[must_use]
    pub fn tool_help(&self, agent: &str, id: &str) -> Option<String> {
        self.conn()
            .ok()?
            .query_row(
                "SELECT tool_help FROM sessions WHERE agent = ?1 AND id = ?2",
                [agent, id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
    }

    /// Keep the tool section this conversation opened with, so every later turn re-uses it rather
    /// than rendering a new one. Written once — a second call is a no-op, on the same `IS NULL`
    /// gate and for the same reason as [`record_outcome`](Self::record_outcome): whichever turn
    /// gets there first decides what this conversation's prompt says.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn freeze_tool_help(&self, agent: &str, id: &str, block: &str) -> Result<()> {
        self.conn()?
            .execute(
                "UPDATE sessions SET tool_help = ?3
                 WHERE agent = ?1 AND id = ?2 AND tool_help IS NULL",
                rusqlite::params![agent, id, block],
            )
            .map_err(|e| db::sql_err("freeze the tool section of", e))?;
        Ok(())
    }

    /// The runner's scratch space for this session, as it is on disk right now.
    #[must_use]
    pub fn runner_state(&self, agent: &str, id: &str) -> Option<serde_json::Value> {
        let conn = self.conn().ok()?;
        let text: Option<String> = conn
            .query_row(
                "SELECT runner_state FROM sessions WHERE agent = ?1 AND id = ?2",
                [agent, id],
                |row| row.get(0),
            )
            .ok()?;
        text.and_then(|t| serde_json::from_str(&t).ok())
    }

    /// Replace the runner's scratch space. Opaque here: the store never looks inside it, and never
    /// merges — a runner rewrites its whole slot, which is the only way it can be sure what is in it.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] when the session is gone (a runner writing into a deleted session
    /// should hear about it rather than resurrect a record with nothing in it), and database errors.
    pub fn set_runner_state(&self, agent: &str, id: &str, value: serde_json::Value) -> Result<()> {
        let changed = self
            .conn()?
            .execute(
                "UPDATE sessions SET runner_state = ?3 WHERE agent = ?1 AND id = ?2",
                rusqlite::params![agent, id, value.to_string()],
            )
            .map_err(|e| db::sql_err("write runner state to", e))?;
        if changed == 0 {
            return Err(Error::NotFound(format!("{agent}: no session {id}")));
        }
        Ok(())
    }

    /// Delete a session outright: its row, its messages, and every file it owns.
    ///
    /// Returns whether there was a session there to delete, so a double-click on Delete is
    /// idempotent rather than an error.
    ///
    /// **Stop it first.** The store cannot signal anything — that is the runner's half — so a child
    /// still running here will happily keep writing into a slot nothing is reading. Ordering is the
    /// caller's to get right.
    ///
    /// # Errors
    /// Returns database errors. The file sweep is best-effort per file.
    pub fn delete(&self, agent: &str, id: &str) -> Result<bool> {
        let conn = self.conn()?;
        // Its images first, while the rows that name them are still readable. They do *not* cascade:
        // an attachment has no foreign key to a session, because it is stored before the session it
        // ends up in exists (see [`attachments`]).
        attachments::delete_for_session(&conn, &self.dir, agent, id);
        // The turns and the queue go with it: they cascade off this row.
        let gone = conn
            .execute(
                "DELETE FROM sessions WHERE agent = ?1 AND id = ?2",
                [agent, id],
            )
            .map_err(|e| db::sql_err("delete a session from", e))?;
        remove_session_files(&self.agent_dir(agent), id);
        Ok(gone > 0)
    }

    // ---- attachments ---------------------------------------------------------------

    /// Store an image and hand back the reference a message carries it by.
    ///
    /// Unclaimed until a turn records it: what is uploaded from a composer may never be sent, so the
    /// same call sweeps whatever was abandoned a day ago. That is the only thing that ever creates
    /// an orphan, so it is the cheapest place to notice one.
    ///
    /// # Errors
    /// Returns [`Error::Arguments`] for an unsupported media type or an oversized body, plus I/O and
    /// database errors.
    pub fn put_attachment(
        &self,
        name: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<Attachment> {
        let conn = self.conn()?;
        attachments::sweep_unclaimed(&conn, &self.dir, db::now_ms());
        attachments::put(&conn, &self.dir, name, media_type, bytes)
    }

    /// One attachment's record, or `None` when there is no such id.
    #[must_use]
    pub fn attachment(&self, id: &str) -> Option<Attachment> {
        self.conn().ok().and_then(|conn| attachments::get(&conn, id))
    }

    /// One attachment's bytes, straight off disk.
    ///
    /// # Errors
    /// Returns the I/O error — including a file that is no longer there, which is what a swept or
    /// half-deleted attachment looks like.
    pub fn attachment_bytes(&self, attachment: &Attachment) -> Result<Vec<u8>> {
        Ok(std::fs::read(attachments::path(&self.dir, attachment))?)
    }

    /// Where one attachment's file is, as an absolute path.
    ///
    /// For the engines that cannot be handed a picture in a request body and are told where to find
    /// it instead. The file is real and stays put for as long as the conversation does, which is
    /// what makes naming it in a prompt honest rather than a race.
    #[must_use]
    pub fn attachment_path(&self, attachment: &Attachment) -> PathBuf {
        attachments::path(&self.dir, attachment)
    }

    /// The records for these ids, in the order asked, dropping any that are gone — how a request
    /// carrying attachment ids is turned into the attachments a turn records.
    #[must_use]
    pub fn resolve_attachments(&self, ids: &[String]) -> Vec<Attachment> {
        self.conn()
            .ok()
            .map(|conn| attachments::resolve(&conn, ids))
            .unwrap_or_default()
    }

    /// Keep only the newest [`MAX_SESSIONS`] sessions of `agent`, deleting older ones.
    /// Returns how many were removed.
    ///
    /// `is_live` is the caller's liveness verdict, taken as an argument rather than asked: whether
    /// something is still running is the runner's question, and a store that could answer it would
    /// be back to knowing about pids. A session it vouches for is never pruned however old it is,
    /// so the count kept can exceed the cap while a long run is in flight.
    pub fn prune_old(&self, agent: &str, is_live: impl Fn(&SessionRecord) -> bool) -> usize {
        let sessions = self.list(agent);
        if sessions.len() <= MAX_SESSIONS {
            return 0;
        }
        let mut removed = 0;
        // `list` is newest first, so everything past the cap is what ages out.
        for session in sessions.into_iter().skip(MAX_SESSIONS) {
            if is_live(&session) {
                continue;
            }
            if self.delete(agent, &session.id).unwrap_or(false) {
                removed += 1;
            }
        }
        removed
    }

    // ---- the queue -----------------------------------------------------------------

    /// Put a message at the back of this session's queue, returning its 1-based place in line.
    ///
    /// # Errors
    /// Returns database errors — a message that was not written down was not queued.
    pub fn enqueue(
        &self,
        agent: &str,
        id: &str,
        message: &str,
        images: &[Attachment],
    ) -> Result<usize> {
        let conn = self.conn()?;
        queue::enqueue(&conn, agent, id, message, images)
    }

    /// Take the head of the queue, or `None` when nothing is waiting. The removal is committed
    /// before the message is handed back, so a message that fails to launch is not retried for ever.
    ///
    /// # Errors
    /// Returns database errors, with the queue left as it was.
    pub fn dequeue(&self, agent: &str, id: &str) -> Result<Option<QueuedMessage>> {
        let conn = self.conn()?;
        queue::dequeue(&conn, agent, id)
    }

    /// Take the head of the queue **and record it as a question**, or `None` when nothing is
    /// waiting — how a turn that is still running hears something said to it since it began.
    ///
    /// The difference from [`dequeue`](Self::dequeue) is who writes the message down. A dequeued
    /// message is written down by the launch that follows it; there is no launch here, so this
    /// writes it itself, in the same transaction as the removal.
    ///
    /// # Errors
    /// Returns database errors and [`Error::NotFound`] for a session deleted mid-turn. The message
    /// stays queued in either case.
    pub fn take_queued_as_turn(&self, agent: &str, id: &str) -> Result<Option<QueuedMessage>> {
        let conn = self.conn()?;
        queue::take_as_turn(&conn, agent, id)
    }

    /// How many messages are waiting.
    #[must_use]
    pub fn queue_len(&self, agent: &str, id: &str) -> usize {
        self.conn()
            .ok()
            .map_or(0, |conn| queue::len(&conn, agent, id))
    }

    /// Which of `agent`'s sessions have anything waiting at all.
    ///
    /// One query for the whole agent, because the caller is a listing: advancing a queue is decided
    /// per session, but *whether there is one to advance* was being asked per session too, and
    /// nothing is waiting in nearly every case. That made the common answer — "no queues anywhere" —
    /// cost a round trip per session on every poll.
    ///
    /// A pre-filter and nothing more. Whoever acts on it re-checks under the turn gate, so a set
    /// that went stale between the two is not a correctness problem.
    #[must_use]
    pub fn sessions_with_queue(&self, agent: &str) -> std::collections::HashSet<String> {
        let Ok(conn) = self.conn() else {
            return std::collections::HashSet::new();
        };
        let Ok(mut stmt) =
            conn.prepare_cached("SELECT DISTINCT session FROM queue WHERE agent = ?1")
        else {
            return std::collections::HashSet::new();
        };
        stmt.query_map([agent], |row| row.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// The messages waiting, oldest first — the order they will be asked in.
    #[must_use]
    pub fn queued(&self, agent: &str, id: &str) -> Vec<QueuedMessage> {
        self.conn()
            .ok()
            .map(|conn| queue::load(&conn, agent, id))
            .unwrap_or_default()
    }

    /// Drop the queued message at `index`. Returns whether there was one there to drop.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn unqueue(&self, agent: &str, id: &str, index: usize) -> Result<bool> {
        let conn = self.conn()?;
        queue::unqueue(&conn, agent, id, index)
    }

    /// Forget everything waiting behind this session — what stopping its current answer does.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn clear_queue(&self, agent: &str, id: &str) -> Result<()> {
        let conn = self.conn()?;
        queue::clear(&conn, agent, id)
    }

    // ---- questions -----------------------------------------------------------------

    /// Write down what this conversation is blocked on, returning the stored ask.
    ///
    /// # Errors
    /// Returns [`Error::Arguments`] for a malformed request or one made while another ask is still
    /// pending, and database errors. See [`questions`](self::questions) for the whole contract.
    pub fn ask(&self, agent: &str, id: &str, req: &AskRequest) -> Result<Ask> {
        let conn = self.conn()?;
        questions::register(&conn, agent, id, req)
    }

    /// Settle an ask — `ask_id` naming one, or `None` taking whatever this conversation has pending
    /// (what an ordinary typed reply means). `None` back means someone else settled it first.
    ///
    /// # Errors
    /// Returns database and encoding errors.
    pub fn resolve_question(
        &self,
        agent: &str,
        id: &str,
        ask_id: Option<&str>,
        answer: &Answer,
    ) -> Result<Option<Ask>> {
        let conn = self.conn()?;
        questions::resolve(&conn, agent, id, ask_id, answer)
    }

    /// What this conversation is waiting on, if it is waiting on anything.
    #[must_use]
    pub fn pending_question(&self, agent: &str, id: &str) -> Option<Ask> {
        let conn = self.conn().ok()?;
        questions::pending(&conn, agent, id)
    }

    /// Every ask this conversation has made, oldest first.
    #[must_use]
    pub fn question_history(&self, agent: &str, id: &str) -> Vec<Ask> {
        self.conn()
            .ok()
            .map(|conn| questions::history(&conn, agent, id))
            .unwrap_or_default()
    }

    /// Every unanswered ask in the whole store, oldest first — the inbox's one query.
    #[must_use]
    pub fn all_pending_questions(&self) -> Vec<Ask> {
        self.conn()
            .ok()
            .map(|conn| questions::all_pending(&conn))
            .unwrap_or_default()
    }

    /// Drop every unanswered ask of an agent, returning how many went — what deleting the agent
    /// takes with it. Settled ones stay with the transcript that explains them.
    pub fn forget_questions(&self, agent: &str) -> usize {
        self.conn()
            .ok()
            .map_or(0, |conn| questions::forget_agent(&conn, agent))
    }

    /// Every pending ask past its deadline, paired with the default it should be settled with.
    #[must_use]
    pub fn overdue_questions(&self, now_ms: u64) -> Vec<(Ask, Answer)> {
        self.conn()
            .ok()
            .map(|conn| questions::overdue(&conn, now_ms))
            .unwrap_or_default()
    }

    // ---- goals ---------------------------------------------------------------------

    /// Write down what this conversation is for, returning the stored goal.
    ///
    /// # Errors
    /// [`Error::Arguments`] for empty or oversized text; database errors otherwise. See
    /// [`goals`](self::goals) for why nothing else here can fail.
    pub fn create_goal(&self, agent: &str, id: &str, text: &str, set_by: SetBy) -> Result<Goal> {
        let conn = self.conn()?;
        goals::create(&conn, agent, id, text, set_by)
    }

    /// Reword an open goal, returning it as it now reads. `None` when no goal has that id.
    ///
    /// # Errors
    /// [`Error::Arguments`] for empty or oversized text; database errors otherwise.
    pub fn edit_goal(&self, goal_id: &str, text: &str) -> Result<Option<Goal>> {
        let conn = self.conn()?;
        goals::edit(&conn, goal_id, text)
    }

    /// Close a goal as met or given up. Never refused: a goal already closed answers
    /// [`GoalClosed::Already`], an id nobody minted answers [`GoalClosed::Unknown`].
    ///
    /// # Errors
    /// Returns database errors only.
    pub fn close_goal(&self, goal_id: &str, state: GoalState, note: &str) -> Result<GoalClosed> {
        let conn = self.conn()?;
        goals::close(&conn, goal_id, state, note)
    }

    /// Stamp every open goal of a conversation as just put to it, and count the nudge.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn mark_goals_nudged(&self, agent: &str, id: &str, at: u64) -> Result<()> {
        let conn = self.conn()?;
        goals::mark_nudged(&conn, agent, id, at)
    }

    /// One goal by id, wherever it lives.
    ///
    /// # Errors
    /// Returns database errors.
    pub fn goal(&self, goal_id: &str) -> Result<Option<Goal>> {
        let conn = self.conn()?;
        goals::by_id(&conn, goal_id)
    }

    /// Every goal of one conversation, oldest first — open and closed alike.
    #[must_use]
    pub fn goals(&self, agent: &str, id: &str) -> Vec<Goal> {
        self.conn()
            .ok()
            .map(|conn| goals::for_conversation(&conn, agent, id))
            .unwrap_or_default()
    }

    /// The goals of one conversation still being worked toward, oldest first.
    #[must_use]
    pub fn open_goals(&self, agent: &str, id: &str) -> Vec<Goal> {
        let mut open = self.goals(agent, id);
        open.retain(Goal::open);
        open
    }

    /// Every open goal in the whole store, oldest first — the sweep's one query.
    #[must_use]
    pub fn all_open_goals(&self) -> Vec<Goal> {
        self.conn()
            .ok()
            .map(|conn| goals::all_open(&conn))
            .unwrap_or_default()
    }

    /// Drop every open goal of an agent, returning how many went. Closed ones stay with the
    /// transcript that explains them.
    pub fn forget_goals(&self, agent: &str) -> usize {
        self.conn()
            .ok()
            .map_or(0, |conn| goals::forget_agent(&conn, agent))
    }

    // ---- the transcript ------------------------------------------------------------

    /// Record a turn, where it stays.
    ///
    /// This is also the only thing that moves a session's `last_activity`, which is the point of
    /// keeping it here rather than deriving it: a listing sorted by when a conversation last *spoke*
    /// must not move because the conversation was read, spooled into, or had a finished answer
    /// committed by the act of opening it. Only a message counts, and only a message passes here.
    ///
    /// The two view-only flags ([`pending`](Turn::pending), [`queued`](Turn::queued)) are cleared on
    /// the way in and the moment is filled in if unset — see [`transcript::append`] for why. Build
    /// the turn with [`user_turn`] / [`assistant_turn`] rather than by hand.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] when the session is gone — a runner that finished answering a
    /// conversation somebody deleted mid-turn should hear about it, not leave orphaned turns behind
    /// for a session nothing lists — and database errors.
    pub fn append_turn(&self, agent: &str, id: &str, turn: Turn) -> Result<()> {
        let conn = self.conn()?;
        transcript::append(&conn, agent, id, turn)
    }

    /// The recorded turns, oldest first — what was actually said, with nothing synthesized.
    ///
    /// This is what an engine that reconstructs its own history replays, and it is deliberately the
    /// plain read: called from inside a running turn, the fuller [`transcript`](Self::transcript)
    /// would splice in that very turn's own empty answer.
    #[must_use]
    pub fn turns(&self, agent: &str, id: &str) -> Vec<Turn> {
        self.conn()
            .ok()
            .map(|conn| transcript::load(&conn, agent, id))
            .unwrap_or_default()
    }

    /// The whole conversation as a reader sees it: the recorded turns, the answer being written
    /// right now, and the questions still waiting behind it.
    ///
    /// `live` is the in-flight answer, **already parsed** by the runner whose engine produced it —
    /// the store never reads a wire format. It is spliced in only behind an unanswered question, so
    /// a poller that keeps handing in the same parse after the answer was committed sees it once
    /// rather than twice; `running` then decides whether it reads as still streaming.
    #[must_use]
    pub fn transcript(
        &self,
        agent: &str,
        id: &str,
        live: Option<TurnContent>,
        running: bool,
    ) -> Vec<Turn> {
        transcript::view(
            self.turns(agent, id),
            live,
            running,
            self.queued(agent, id),
        )
    }
}

// ---- internals ---------------------------------------------------------------------

/// Remove every file a session owns.
///
/// By prefix rather than a list of extensions: a runner is free to keep its own sidecars beside the
/// log, and a delete that swept a fixed list would leave the newest of them behind every time
/// somebody invented one.
fn remove_session_files(dir: &Path, id: &str) {
    let prefix = format!("{id}.");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Session;

    /// Just what was typed. Most cases here are about the order of the line rather than about what
    /// rode along with a message, and comparing whole [`QueuedMessage`]s would say so in every one.
    fn texts(queued: Vec<QueuedMessage>) -> Vec<String> {
        queued.into_iter().map(|m| m.text).collect()
    }

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        // A previous case on this thread may have left a connection open on a database at this very
        // path, which has since been deleted; reusing it would read the ghost.
        db::forget_connections();
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    fn now_ms() -> u64 {
        u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("after the epoch")
                .as_millis(),
        )
        .expect("fits")
    }

    /// Lay down a session at a chosen id, so a test can control when it "started" without waiting.
    fn seed(store: &SessionStore, agent: &str, id: &str, message: &str) {
        std::fs::create_dir_all(store.agent_dir(agent)).unwrap();
        let started = record::started_at(id);
        db::conn(&store.db_path())
            .unwrap()
            .execute(
                "INSERT INTO sessions
                   (agent, id, backend, cwd, message, started_at, last_activity, hidden)
                 VALUES (?1, ?2, 'harness:adi', '/tmp', ?3, ?4, ?4, 0)",
                rusqlite::params![agent, id, message, started],
            )
            .unwrap();
    }

    /// The ending is recorded by whoever notices it first, and by nobody after that. Several
    /// processes watch the same run — the app's poll, a CLI listing, a trigger's child — and the
    /// announcement hangs off this returning `true`, so a second writer must be told `false`
    /// rather than allowed to overwrite and re-announce.
    #[test]
    fn an_outcome_is_written_once_and_the_first_writer_is_told_so() {
        let store = scratch("outcome");
        let run = store
            .create("solver", Backend::ProcessClaude, "/tmp/work", "go")
            .expect("create");
        assert!(
            store.get("solver", &run.id).expect("get").outcome.is_none(),
            "a running session has no outcome yet"
        );

        let finished = RunOutcome {
            terminal_reason: Some("completed".into()),
            result_head: "Filed three tasks.".into(),
            duration_ms: Some(23_169),
            ..RunOutcome::default()
        };
        assert!(
            store
                .record_outcome("solver", &run.id, &finished)
                .expect("record"),
            "the first writer records it"
        );

        let later = RunOutcome {
            terminal_reason: Some("api_error".into()),
            is_error: true,
            ..RunOutcome::default()
        };
        assert!(
            !store
                .record_outcome("solver", &run.id, &later)
                .expect("record again"),
            "the second writer is refused"
        );

        let stored = store.get("solver", &run.id).expect("get").outcome;
        assert_eq!(stored, Some(finished), "the first outcome is the one kept");

        let listed = store.list("solver");
        assert_eq!(
            listed[0].outcome.as_ref().and_then(|o| o.terminal_reason.as_deref()),
            Some("completed"),
        );
    }

    #[test]
    fn a_created_session_round_trips_and_lists_newest_first() {
        let store = scratch("crud");
        assert!(store.list("solver").is_empty(), "an unknown agent is empty");
        assert!(store.get("solver", "nope").is_none());

        let first = store
            .create("solver", Backend::HarnessAdi, "/tmp/work", "first task")
            .expect("create");
        let second = store
            .create("solver", Backend::ProcessClaude, "/tmp/other", "second task")
            .expect("create");
        assert_ne!(first.id, second.id, "each session gets its own id");

        let got = store.get("solver", &first.id).expect("get");
        assert_eq!(got.agent, "solver");
        assert_eq!(got.backend, Backend::HarnessAdi);
        assert_eq!(got.cwd, PathBuf::from("/tmp/work"));
        assert_eq!(got.message, "first task");
        assert!(got.started_at > 0);
        assert!(got.last_activity >= got.started_at);
        assert!(!got.hidden);
        assert!(got.runner_state.is_none());

        let listed = store.list("solver");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id, "newest first");
        assert_eq!(listed[1].id, first.id);
        assert_eq!(listed[0].message, "second task");

        assert!(store.list("other").is_empty());
        assert!(store.get("other", &first.id).is_none());
        assert_eq!(store.agents(), ["solver"]);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The bug the backend-as-a-column exists to fix: two sessions of one agent run by *different*
    /// engines sit side by side, and nothing about the backend decides where they are found.
    /// Re-pointing the agent at another engine cannot make either disappear.
    #[test]
    fn changing_the_backend_cannot_lose_a_session() {
        let store = scratch("backends");
        let harness = store
            .create("solver", Backend::HarnessClaudeSdk, "/tmp", "under harness")
            .expect("create");
        let process = store
            .create("solver", Backend::ProcessCodex, "/tmp", "under process")
            .expect("create");

        let listed = store.list("solver");
        assert_eq!(listed.len(), 2, "both engines' sessions are one history");
        assert_eq!(
            store.get("solver", &harness.id).map(|s| s.backend),
            Some(Backend::HarnessClaudeSdk),
        );
        assert_eq!(
            store.get("solver", &process.id).map(|s| s.backend),
            Some(Backend::ProcessCodex),
        );

        assert!(store.db_path().is_file());
        assert!(!store.dir().join("harness").exists());
        assert!(!store.dir().join("process").exists());
        assert_eq!(
            store.log_path("solver", &harness.id),
            store.dir().join("solver").join(format!("{}.log", harness.id)),
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Hiding is a flag and nothing else: the session stays in the history with its task intact, and
    /// flagging it never reads as the session having just moved.
    #[test]
    fn hiding_a_session_only_flags_it() {
        let store = scratch("hidden");
        let now = now_ms();
        let id = format!("{:013}-0001", now - 3_600_000);
        seed(&store, "talker", &id, "some task");
        let spoke = now - 1_800_000;
        store
            .append_turn("talker", &id, Turn { at: spoke, ..user_turn("said then") })
            .expect("append");

        let before = store.get("talker", &id).expect("listed");
        assert!(!before.hidden, "a session nobody hid is not hidden");
        assert_eq!(before.last_activity, spoke);

        assert!(
            store.set_hidden("talker", &id, true).expect("hide"),
            "an existing session is there to flag",
        );
        let hidden = store.get("talker", &id).expect("still listed");
        assert!(hidden.hidden, "the flag round-trips");
        assert_eq!(hidden.message, before.message, "the task is not lost");
        assert_eq!(hidden.started_at, before.started_at);
        assert_eq!(
            hidden.last_activity, before.last_activity,
            "hiding is not activity",
        );
        assert_eq!(store.list("talker").len(), 1, "and it is still listed");

        assert!(store.set_hidden("talker", &id, false).expect("unhide"));
        assert!(!store.get("talker", &id).expect("listed").hidden);
        assert!(
            !store.set_hidden("talker", "0000000000001-0000", true).expect("absent"),
            "a session that isn't there is nothing to flag",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Activity is the last thing *said*, not the clock the session started on — that is what makes
    /// a long, quiet conversation sort below one answered a minute ago — and it never precedes the
    /// start.
    #[test]
    fn activity_is_the_last_turn_but_never_precedes_the_start() {
        let store = scratch("activity");
        let now = now_ms();

        // Started an hour ago, spoke a minute ago: it must read as active a minute ago.
        let quiet = format!("{:013}-0001", now - 3_600_000);
        seed(&store, "talker", &quiet, "old but busy");
        let spoke = now - 60_000;
        store
            .append_turn("talker", &quiet, Turn { at: spoke, ..user_turn("still here") })
            .expect("append");

        // An id claiming to start in an hour: what it said is already older than that.
        let future = format!("{:013}-0002", now + 3_600_000);
        seed(&store, "talker", &future, "from the future");
        store
            .append_turn("talker", &future, Turn { at: now, ..user_turn("said now") })
            .expect("append");

        let busy = store.get("talker", &quiet).expect("listed");
        assert_eq!(busy.started_at, now - 3_600_000, "start comes from the id");
        assert_eq!(busy.last_activity, spoke, "activity is the turn's own moment");

        let odd = store.get("talker", &future).expect("listed");
        assert_eq!(
            odd.last_activity, odd.started_at,
            "a turn older than the start never drags activity backwards",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The regression this column exists for: everything a session *writes* while it works — its
    /// log, its queue, whatever sidecar a runner spools — leaves the rail's ordering alone. Only a
    /// turn moves it, so a chat cannot jump to the top for having been read, spooled into, or hidden.
    #[test]
    fn only_a_turn_counts_as_activity() {
        let store = scratch("only-turns");
        let now = now_ms();
        let dir = store.agent_dir("talker");
        let id = format!("{:013}-0001", now - 3_600_000);
        seed(&store, "talker", &id, "answered a while back");
        let answered = now - 1_800_000;
        store
            .append_turn("talker", &id, Turn { at: answered, ..user_turn("what is it") })
            .expect("append");

        std::fs::write(record::log_path(&dir, &id), "the engine spooling").unwrap();
        std::fs::write(dir.join(format!("{id}.invented-later")), "a runner's own").unwrap();
        store.enqueue("talker", &id, "waiting", &[]).expect("enqueue");
        store.set_hidden("talker", &id, true).expect("hide");
        store
            .set_runner_state("talker", &id, serde_json::json!({ "pid": 4711 }))
            .expect("park state");

        assert_eq!(
            store.get("talker", &id).expect("listed").last_activity,
            answered,
            "none of that is the conversation saying anything",
        );

        store
            .append_turn("talker", &id, Turn { at: now, ..user_turn("still there?") })
            .expect("append");
        assert_eq!(store.get("talker", &id).expect("listed").last_activity, now);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Deleting takes *everything* a session owns — its row, its messages, and its files, including
    /// a sidecar this module has never heard of, which is the whole reason the file sweep goes by
    /// prefix — and nothing of its neighbour's.
    #[test]
    fn deleting_a_session_leaves_none_of_its_files_and_all_of_its_neighbours() {
        let store = scratch("delete");
        let doomed = store
            .create("chat", Backend::HarnessAdi, "/tmp", "goes")
            .expect("create");
        let keeper = store
            .create("chat", Backend::HarnessAdi, "/tmp", "stays")
            .expect("create");
        let dir = store.agent_dir("chat");
        for id in [&doomed.id, &keeper.id] {
            std::fs::write(record::log_path(&dir, id), "output").unwrap();
            // A sidecar invented by some runner: swept by prefix, unknown to this module.
            std::fs::write(dir.join(format!("{id}.invented-later")), "{}\n").unwrap();
            store.append_turn("chat", id, user_turn("said")).expect("append");
        }
        store.enqueue("chat", &doomed.id, "waiting", &[]).expect("enqueue");

        assert!(store.delete("chat", &doomed.id).expect("delete"));
        for path in [
            record::log_path(&dir, &doomed.id),
            dir.join(format!("{}.invented-later", doomed.id)),
        ] {
            assert!(!path.exists(), "{} survived the delete", path.display());
        }
        assert!(store.get("chat", &doomed.id).is_none());
        assert!(
            store.turns("chat", &doomed.id).is_empty() && store.queued("chat", &doomed.id).is_empty(),
            "its messages cascade off the row",
        );

        let left = store.list("chat");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, keeper.id);
        assert!(dir.join(format!("{}.invented-later", keeper.id)).exists());
        assert_eq!(store.turns("chat", &keeper.id).len(), 1);

        assert!(!store.delete("chat", &doomed.id).expect("delete again"));

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Pruning keeps the newest, sweeps the rest — and never touches one the caller says is still
    /// running, however old it is.
    #[test]
    fn pruning_keeps_the_newest_and_spares_whatever_is_still_live() {
        let store = scratch("prune");
        let dir = store.agent_dir("busy");
        let base = now_ms() - 10_000_000;
        let ids: Vec<String> = (0..MAX_SESSIONS + 5)
            .map(|n| {
                let id = format!("{:013}-{n:04}", base + n as u64);
                seed(&store, "busy", &id, "task");
                std::fs::write(record::log_path(&dir, &id), "output").unwrap();
                id
            })
            .collect();
        assert_eq!(store.list("busy").len(), MAX_SESSIONS + 5);

        let store_small = scratch("prune-small");
        seed(&store_small, "busy", "0000000000001-0000", "task");
        assert_eq!(store_small.prune_old("busy", |_| false), 0);
        let _ = std::fs::remove_dir_all(store_small.dir());

        let oldest = ids[0].clone();
        let removed = store.prune_old("busy", |s| s.id == oldest);
        assert_eq!(removed, 4, "five over the cap, one of them alive");
        let left = store.list("busy");
        assert_eq!(left.len(), MAX_SESSIONS + 1);
        assert!(
            left.iter().any(|s| s.id == oldest),
            "a live session is never pruned",
        );
        assert!(
            left.iter().any(|s| s.id == ids[MAX_SESSIONS + 4]),
            "the newest is kept",
        );
        for id in &ids[1..5] {
            assert!(store.get("busy", id).is_none(), "{id} should be gone");
            assert!(!record::log_path(&dir, id).exists());
        }

        assert_eq!(store.prune_old("busy", |_| false), 1);
        assert_eq!(store.list("busy").len(), MAX_SESSIONS);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn the_queue_is_per_session_and_answered_in_order() {
        let store = scratch("queue");
        let a = store
            .create("chat", Backend::HarnessAdi, "/tmp", "first chat")
            .expect("create");
        let b = store
            .create("chat", Backend::HarnessAdi, "/tmp", "second chat")
            .expect("create");

        assert_eq!(store.queue_len("chat", &a.id), 0);
        assert!(store.dequeue("chat", &a.id).expect("dequeue").is_none());

        assert_eq!(store.enqueue("chat", &a.id, "one", &[]).expect("enqueue"), 1);
        assert_eq!(store.enqueue("chat", &a.id, "two", &[]).expect("enqueue"), 2);
        assert_eq!(store.enqueue("chat", &b.id, "elsewhere", &[]).expect("enqueue"), 1);
        assert_eq!(store.queue_len("chat", &a.id), 2);
        assert_eq!(texts(store.queued("chat", &a.id)), ["one", "two"]);
        assert_eq!(
            texts(store.queued("chat", &b.id)),
            ["elsewhere"],
            "one session's queue is not another's",
        );

        assert_eq!(
            store.dequeue("chat", &a.id).expect("dequeue").map(|m| m.text).as_deref(),
            Some("one"),
        );
        assert_eq!(texts(store.queued("chat", &a.id)), ["two"]);

        assert_eq!(store.enqueue("chat", &a.id, "three", &[]).expect("enqueue"), 2);
        assert!(store.unqueue("chat", &a.id, 0).expect("unqueue"));
        assert_eq!(texts(store.queued("chat", &a.id)), ["three"]);
        assert!(!store.unqueue("chat", &a.id, 9).expect("past the end"));

        store.clear_queue("chat", &a.id).expect("clear");
        assert_eq!(store.queue_len("chat", &a.id), 0);
        assert_eq!(
            texts(store.queued("chat", &b.id)),
            ["elsewhere"],
            "clearing one queue leaves the other",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A turn that hears something mid-answer has no launch to write the message down for it, so
    /// taking it *is* asking it: the message leaves the queue and enters the transcript together, or
    /// neither happens and it is offered again.
    #[test]
    fn a_message_taken_by_a_running_turn_is_asked_in_the_same_breath() {
        let store = scratch("take-queued");
        let session = store
            .create("chat", Backend::HarnessAdi, "/tmp", "start on the parser")
            .expect("create");
        let id = &session.id;
        store
            .append_turn("chat", id, user_turn("start on the parser"))
            .expect("the opening question");

        assert!(
            store.take_queued_as_turn("chat", id).expect("empty").is_none(),
            "nothing waiting is not an error",
        );

        store.enqueue("chat", id, "also handle CRLF", &[]).expect("enqueue");
        store.enqueue("chat", id, "and add a test", &[]).expect("enqueue");
        assert_eq!(
            store
                .take_queued_as_turn("chat", id)
                .expect("take")
                .map(|m| m.text)
                .as_deref(),
            Some("also handle CRLF"),
        );

        let turns = store.turns("chat", id);
        assert_eq!(
            turns.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["start on the parser", "also handle CRLF"],
            "the message it heard is a question it was asked",
        );
        assert!(turns.iter().all(|t| t.role == "user" && !t.queued));
        assert_eq!(
            texts(store.queued("chat", id)),
            ["and add a test"],
            "only the head was taken; the rest is still waiting",
        );

        // A session deleted out from under a running turn takes the message with it rather than
        // recording a turn nothing lists — and says so, instead of losing it silently.
        store.delete("chat", id).expect("delete");
        assert!(matches!(
            store.take_queued_as_turn("chat", id),
            Err(Error::NotFound(_)) | Ok(None)
        ));

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The store owns a session's turns, so a reader asks it — not a runner — for the conversation:
    /// what was recorded, what is being said now, and what is still waiting. Only the first of those
    /// is durable.
    #[test]
    fn the_transcript_is_what_was_recorded_plus_what_is_still_happening() {
        let store = scratch("transcript");
        let session = store
            .create("chat", Backend::HarnessAdi, "/tmp", "set the port")
            .expect("create");
        let id = &session.id;
        assert!(store.turns("chat", id).is_empty(), "nothing said yet");

        store
            .append_turn("chat", id, user_turn("set the port"))
            .expect("append");
        let live = TurnContent {
            text: "looking".into(),
            steps: vec![crate::progress::Step::Message {
                text: "one moment".into(),
            }],
            metrics: None,
        };
        store.enqueue("chat", id, "and restart it", &[]).expect("enqueue");
        store.enqueue("chat", id, "then tell me", &[]).expect("enqueue");

        let view = store.transcript("chat", id, Some(live.clone()), true);
        assert_eq!(view.len(), 4);
        assert!(view[1].pending, "the answer is still being written");
        assert_eq!(view[1].text, "looking");
        assert_eq!(
            view[2..].iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["and restart it", "then tell me"],
        );
        assert!(view[2..].iter().all(|t| t.queued));
        assert_eq!(store.turns("chat", id).len(), 1, "none of that is recorded");

        store
            .append_turn("chat", id, assistant_turn(&live))
            .expect("append");
        let settled = store.transcript("chat", id, Some(live), false);
        assert_eq!(settled.len(), 4, "the answer, once, then the queue");
        assert!(!settled[1].pending);
        let turns = store.turns("chat", id);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, ROLE_USER, "oldest first");
        assert_eq!(turns[1].steps.len(), 1, "the timeline round-trips");

        let other = store
            .create("chat", Backend::HarnessAdi, "/tmp", "elsewhere")
            .expect("create");
        assert!(store.turns("chat", &other.id).is_empty());
        assert!(store.delete("chat", id).expect("delete"));
        assert!(
            store.append_turn("chat", id, user_turn("too late")).is_err(),
            "a deleted session is not resurrected by a late turn",
        );
        assert!(store.turns("chat", id).is_empty());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The listing's two shortcuts, and the line they must not cross.
    ///
    /// `sessions_with_queue` answers for a whole agent at once, because the per-session version of
    /// the question was being asked a few hundred times per poll to hear "nothing" every time. And a
    /// view built from a listed record answers `state()` from the copy that record carried, which is
    /// what a listing wants and what a *writer* must never take.
    #[test]
    fn a_listing_asks_once_for_the_queues_and_reuses_the_state_it_read() {
        let store = scratch("listing");
        let quiet = store
            .create("chat", Backend::HarnessAdi, "/tmp", "nothing waiting")
            .expect("create");
        let busy = store
            .create("chat", Backend::HarnessAdi, "/tmp", "two waiting")
            .expect("create");

        assert!(
            store.sessions_with_queue("chat").is_empty(),
            "no queues anywhere is the common answer, and it costs one query",
        );
        store.enqueue("chat", &busy.id, "one", &[]).expect("enqueue");
        store.enqueue("chat", &busy.id, "two", &[]).expect("enqueue");
        assert_eq!(
            store.sessions_with_queue("chat"),
            std::collections::HashSet::from([busy.id.clone()]),
            "only the session that has something waiting, once however much is in it",
        );
        assert!(store.sessions_with_queue("elsewhere").is_empty());

        store.clear_queue("chat", &busy.id).expect("clear");
        assert!(store.sessions_with_queue("chat").is_empty());

        store
            .set_runner_state("chat", &quiet.id, serde_json::json!({ "pid": 4711 }))
            .expect("park state");
        let record = store.get("chat", &quiet.id).expect("listed");
        let listed = store.session_as_listed(&record);
        let fresh = store.session("chat", &quiet.id);
        assert_eq!(listed.state(), Some(serde_json::json!({ "pid": 4711 })));

        // ...and goes on answering that after another writer has moved on, which is exactly why it
        // is for reads. The plain view is the one that sees the change.
        store
            .set_runner_state("chat", &quiet.id, serde_json::json!({ "pid": 9999 }))
            .expect("another writer");
        assert_eq!(
            listed.state(),
            Some(serde_json::json!({ "pid": 4711 })),
            "a snapshot is a snapshot",
        );
        assert_eq!(
            fresh.state(),
            Some(serde_json::json!({ "pid": 9999 })),
            "and the default view re-reads",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The row is what says a session exists — a stray log is not one.
    ///
    /// The file store answered otherwise, and had to: it wrote the sidecar and the log separately, so
    /// a crash between them left output that nothing could list. Here the row is committed before a
    /// runner is ever handed the log path, so a log without a row is not a lost session, it is
    /// somebody else's file in the directory. It is swept if its session is later deleted, and
    /// otherwise left alone.
    #[test]
    fn a_stray_log_is_not_a_session() {
        let store = scratch("orphan");
        let dir = store.agent_dir("solver");
        std::fs::create_dir_all(&dir).unwrap();
        let id = "0000000054321-0000";
        std::fs::write(record::log_path(&dir, id), "output nobody recorded").unwrap();

        assert!(store.list("solver").is_empty());
        assert!(store.get("solver", id).is_none());
        assert!(!store.set_hidden("solver", id, true).expect("hide"));
        assert!(!store.delete("solver", id).expect("delete"));
        assert!(
            store.set_runner_state("solver", id, serde_json::json!({})).is_err(),
            "and a runner writing into it is told, not quietly given a row",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
