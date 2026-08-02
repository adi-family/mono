//! The session store: everything durable about a session that is *not* the process running it.
//!
//! A runner is a process manager, so it keeps nothing. The record, the queue, the hidden flag, the
//! log, and the runner's own opaque state slot all live here — see `docs/agent-runner.md` for the
//! layering this is one third of.
//!
//! # Flat, and backend-agnostic
//!
//! ```text
//! <sessions_dir>/<agent>/<session_id>.meta.json         the record
//! <sessions_dir>/<agent>/<session_id>.log               the raw output a runner spools into
//! <sessions_dir>/<agent>/<session_id>.queue.json        what is waiting to be said next
//! <sessions_dir>/<agent>/<session_id>.transcript.jsonl  what was said, one turn per line
//! ```
//!
//! Note what is *not* in that path: the executor. It used to be a directory level
//! (`<sessions>/<process|harness>/<agent>/…`), which quietly made a stored field decide where
//! history was read from — change an agent's `backend` and every run it had ever done vanished,
//! because the listing looked under the new executor's directory while the files sat under the old
//! one's. They could not be listed, peeked at, stopped, or deleted again. Keeping the backend inside
//! the record instead means a session is found by the two things that never change, who it belongs
//! to and what it is called, and re-pointing an agent at another engine leaves its history exactly
//! where it was.
//!
//! A session owns the whole `<id>.*` namespace of its agent directory — including sidecars a runner
//! invents and this module has never heard of — which is why deleting and pruning sweep by prefix.

mod migrate;
mod queue;
mod record;
mod session;
mod transcript;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::Backend;
use crate::error::{Error, Result};
use crate::progress::TurnContent;

pub use record::SessionRecord;
pub use session::SessionRef;
pub use transcript::{Turn, assistant_turn, user_turn};

/// The role a question carries — what the agent layer reads to tell an unanswered turn from a
/// settled one before it commits an answer behind it.
pub(crate) use transcript::ROLE_USER;

/// How many sessions to keep per agent before [`prune_old`](SessionStore::prune_old) sweeps the
/// oldest finished ones.
pub const MAX_SESSIONS: usize = 50;

/// The sessions on disk, under one root.
///
/// Cheap to clone and safe to hold: it owns a path and no cached state, so two of them over the same
/// root are the same store and cannot disagree.
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

    /// Where one agent's sessions are filed — the only path segment between the root and a session
    /// id, on purpose (see the module docs).
    #[must_use]
    pub fn agent_dir(&self, agent: &str) -> PathBuf {
        self.dir.join(agent)
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
    #[must_use]
    pub fn session(&self, agent: &str, id: &str) -> SessionRef<'_> {
        SessionRef::new(self, agent, id)
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
    /// Returns directory-creation and write errors.
    pub fn create(
        &self,
        agent: &str,
        backend: Backend,
        cwd: impl Into<PathBuf>,
        message: &str,
    ) -> Result<SessionRecord> {
        let dir = self.agent_dir(agent);
        std::fs::create_dir_all(&dir)?;
        let id = record::new_id();
        let session = SessionRecord {
            started_at: record::started_at(&id),
            last_activity: record::started_at(&id),
            id,
            agent: agent.to_string(),
            backend,
            cwd: cwd.into(),
            message: message.to_string(),
            hidden: false,
            runner_state: None,
        };
        record::write(&dir, &session)?;
        Ok(session)
    }

    /// One session's record, or `None` when there is nothing filed under that id.
    ///
    /// A session counts as present when *either* its record or its log is there: a sidecar lost to a
    /// crash must not make a session with real output unreachable.
    #[must_use]
    pub fn get(&self, agent: &str, id: &str) -> Option<SessionRecord> {
        let dir = self.agent_dir(agent);
        if !exists_in(&dir, id) {
            return None;
        }
        let mut session = record::read(&dir, agent, id);
        session.last_activity = last_activity(&dir)
            .get(id)
            .copied()
            .unwrap_or(0)
            .max(session.started_at);
        Some(session)
    }

    /// Every session of `agent`, newest first.
    #[must_use]
    pub fn list(&self, agent: &str) -> Vec<SessionRecord> {
        let dir = self.agent_dir(agent);
        let touched = last_activity(&dir);
        let mut sessions: Vec<SessionRecord> = session_ids(&dir)
            .into_iter()
            .map(|id| {
                let mut session = record::read(&dir, agent, &id);
                // A session that left no readable mtime behind falls back to when it began, so the
                // field is always a moment the session existed rather than the epoch.
                session.last_activity = touched
                    .get(&id)
                    .copied()
                    .unwrap_or(0)
                    .max(session.started_at);
                session
            })
            .collect();
        // By start, not by activity: the rail's own ordering is a view's business, and a listing
        // that reshuffled itself as answers landed would be a different thing to page through.
        // Ids are zero-padded, so the id is the tiebreak that keeps a same-millisecond pair stable.
        sessions.sort_unstable_by(|a, b| {
            b.started_at.cmp(&a.started_at).then_with(|| b.id.cmp(&a.id))
        });
        sessions
    }

    /// Hide (or unhide) a session: a flag in its record, so the choice outlives the tab that made
    /// it. Nothing else changes — it keeps running, keeps its log, and is still listed by everything
    /// that asks for the full history.
    ///
    /// Returns whether there was a session there to flag; an unknown id is `false`, so a stale click
    /// is idempotent rather than an error. Writing the record is deliberately *not* activity — a
    /// hidden session brought back must not have jumped to the top of the rail in the meantime,
    /// which is why the sidecar is the one file [`last_activity`] leaves out.
    ///
    /// # Errors
    /// Returns write errors.
    pub fn set_hidden(&self, agent: &str, id: &str, hidden: bool) -> Result<bool> {
        let dir = self.agent_dir(agent);
        if !exists_in(&dir, id) {
            return Ok(false);
        }
        let mut session = record::read(&dir, agent, id);
        session.hidden = hidden;
        record::write(&dir, &session)?;
        Ok(true)
    }

    /// The runner's scratch space for this session, as it is on disk.
    #[must_use]
    pub fn runner_state(&self, agent: &str, id: &str) -> Option<serde_json::Value> {
        self.get(agent, id).and_then(|s| s.runner_state)
    }

    /// Replace the runner's scratch space. Opaque here: the store never looks inside it, and never
    /// merges — a runner rewrites its whole slot, which is the only way it can be sure what is in it.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] when the session is gone (a runner writing into a deleted session
    /// should hear about it rather than resurrect a record with nothing in it), and write errors.
    pub fn set_runner_state(
        &self,
        agent: &str,
        id: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        let dir = self.agent_dir(agent);
        if !exists_in(&dir, id) {
            return Err(Error::NotFound(format!("{agent}: no session {id}")));
        }
        let mut session = record::read(&dir, agent, id);
        session.runner_state = Some(value);
        record::write(&dir, &session)
    }

    /// Delete a session outright: every file it owns, and only its own.
    ///
    /// Returns whether there was a session there to delete, so a double-click on Delete is
    /// idempotent rather than an error.
    ///
    /// **Stop it first.** The store cannot signal anything — that is the runner's half — so a child
    /// still running here will happily keep writing into a slot nothing is reading. Ordering is the
    /// caller's to get right.
    ///
    /// # Errors
    /// Reserved for future storage backends; the local sweep is best-effort per file.
    pub fn delete(&self, agent: &str, id: &str) -> Result<bool> {
        let dir = self.agent_dir(agent);
        if !exists_in(&dir, id) {
            return Ok(false);
        }
        remove_session_files(&dir, id);
        Ok(true)
    }

    /// Keep only the newest [`MAX_SESSIONS`] sessions of `agent`, deleting older ones' files.
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
        let dir = self.agent_dir(agent);
        let mut removed = 0;
        // `list` is newest first, so everything past the cap is what ages out.
        for session in sessions.into_iter().skip(MAX_SESSIONS) {
            if is_live(&session) {
                continue;
            }
            remove_session_files(&dir, &session.id);
            removed += 1;
        }
        removed
    }

    // ---- the queue -----------------------------------------------------------------

    /// Put a message at the back of this session's queue, returning its 1-based place in line.
    ///
    /// # Errors
    /// Returns write errors — a message that was not written down was not queued.
    pub fn enqueue(&self, agent: &str, id: &str, message: &str) -> Result<usize> {
        queue::enqueue(&self.agent_dir(agent), id, message)
    }

    /// Take the head of the queue, or `None` when nothing is waiting. The removal is persisted
    /// before the message is handed back, so a message that fails to launch is not retried for ever.
    ///
    /// # Errors
    /// Returns write errors, with the queue left as it was.
    pub fn dequeue(&self, agent: &str, id: &str) -> Result<Option<String>> {
        queue::dequeue(&self.agent_dir(agent), id)
    }

    /// How many messages are waiting.
    #[must_use]
    pub fn queue_len(&self, agent: &str, id: &str) -> usize {
        queue::load(&self.agent_dir(agent), id).len()
    }

    /// The messages waiting, oldest first — the order they will be asked in.
    #[must_use]
    pub fn queued(&self, agent: &str, id: &str) -> Vec<String> {
        queue::load(&self.agent_dir(agent), id)
    }

    /// Drop the queued message at `index`. Returns whether there was one there to drop.
    ///
    /// # Errors
    /// Returns write errors.
    pub fn unqueue(&self, agent: &str, id: &str, index: usize) -> Result<bool> {
        queue::unqueue(&self.agent_dir(agent), id, index)
    }

    /// Forget everything waiting behind this session — what stopping its current answer does.
    ///
    /// # Errors
    /// Returns write errors.
    pub fn clear_queue(&self, agent: &str, id: &str) -> Result<()> {
        queue::clear(&self.agent_dir(agent), id)
    }

    // ---- the transcript ------------------------------------------------------------

    /// Record a turn: append it to this session's transcript, where it stays.
    ///
    /// The two view-only flags ([`pending`](Turn::pending), [`queued`](Turn::queued)) are cleared on
    /// the way in and the moment is filled in if unset — see [`transcript::append`] for why. Build
    /// the turn with [`user_turn`] / [`assistant_turn`] rather than by hand.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] when the session is gone — a runner that finished answering a
    /// conversation somebody deleted mid-turn should hear about it, not leave an orphan transcript
    /// behind for a session nothing lists — and write errors.
    pub fn append_turn(&self, agent: &str, id: &str, turn: Turn) -> Result<()> {
        let dir = self.agent_dir(agent);
        if !exists_in(&dir, id) {
            return Err(Error::NotFound(format!("{agent}: no session {id}")));
        }
        transcript::append(&dir, id, turn)
    }

    /// The turns on disk, oldest first — what was actually said, with nothing synthesized.
    ///
    /// This is what an engine that reconstructs its own history replays, and it is deliberately the
    /// plain read: called from inside a running turn, the fuller [`transcript`](Self::transcript)
    /// would splice in that very turn's own empty answer.
    #[must_use]
    pub fn turns(&self, agent: &str, id: &str) -> Vec<Turn> {
        (*transcript::load(&self.agent_dir(agent), id)).clone()
    }

    /// The whole conversation as a reader sees it: the persisted turns, the answer being written
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
        let dir = self.agent_dir(agent);
        transcript::view(&dir, id, live, running, self.queued(agent, id))
    }

    // ---- the old layout ------------------------------------------------------------

    /// Move any sessions still filed the old way — `<sessions>/<process|harness>/<agent>/<id>.*` —
    /// into this store's flat layout, and return how many were moved.
    ///
    /// Idempotent, so it is safe to call on every open: a second run finds nothing and reports `0`.
    /// A session the new layout already has is never overwritten — see [`migrate`] for why its
    /// legacy copy is left standing rather than merged.
    ///
    /// `backend_of` answers which engine an agent runs, because the old path could not: it named
    /// the *executor*, and `process:claude` and `process:codex` shared a directory. The caller reads
    /// that off the agent's manifest; the store has no business opening one.
    ///
    /// # Errors
    /// Returns directory-creation, move, and record-write errors. Sessions already moved stay moved,
    /// and a later run picks up the rest.
    pub fn migrate_legacy(&self, backend_of: impl Fn(&str) -> Backend) -> Result<usize> {
        migrate::legacy(self, &backend_of)
    }
}

// ---- internals ---------------------------------------------------------------------

/// Whether anything is filed under this id: its record, or the log a crash left without one.
fn exists_in(dir: &Path, id: &str) -> bool {
    record::meta_path(dir, id).exists() || record::log_path(dir, id).exists()
}

/// Every session id present in an agent's directory.
///
/// Taken from the record *and* the log, because either can be there alone: a session created but not
/// yet sent to has no log, and one whose sidecar was lost still has output worth listing.
fn session_ids(dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            // Session ids hold no dots (`{millis}-{seq}`), so everything before the first one is
            // the id and everything after it says which of the session's files this is.
            let (id, rest) = name.split_once('.')?;
            (rest == record::META_SUFFIX || rest == record::LOG_SUFFIX).then(|| id.to_string())
        })
        .collect()
}

/// When each session in `dir` last did anything, as unix millis keyed by id.
///
/// A session owns the whole `<id>.*` namespace, and the files it writes as it works — the log, a
/// transcript, whatever a runner spools beside them — are appended to for as long as it is talking.
/// So the newest mtime across its files is the last moment it moved. Sessions whose files carry no
/// readable mtime are simply absent; the caller falls back to their start.
///
/// The record is the one file left out. It is written at creation (which `started_at` already
/// reports) and then only when a *reader* changes its mind — hiding a session, or a runner parking
/// its state slot. Counting that as activity would have a hide read as the session having just
/// moved, and send a chat somebody just put away back to the top of the rail.
fn last_activity(dir: &Path) -> BTreeMap<String, u64> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    let mut newest: BTreeMap<String, u64> = BTreeMap::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some((id, rest)) = name.split_once('.') else {
            continue;
        };
        if rest == record::META_SUFFIX {
            continue;
        }
        let Some(ms) = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| u64::try_from(d.as_millis()).ok())
        else {
            continue;
        };
        let slot = newest.entry(id.to_string()).or_default();
        *slot = (*slot).max(ms);
    }
    newest
}

/// Remove every file a session owns.
///
/// By prefix rather than a list of extensions: a runner is free to keep its own sidecars beside the
/// ones named here, and a delete that swept a fixed list would leave the newest of them behind
/// every time somebody invented one.
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
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use super::*;

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    fn now_ms() -> u64 {
        u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("after the epoch")
                .as_millis(),
        )
        .expect("fits")
    }

    /// Lay down a session at a chosen id, so a test can control when it "started" without waiting.
    fn seed(store: &SessionStore, agent: &str, id: &str, message: &str) {
        let dir = store.agent_dir(agent);
        std::fs::create_dir_all(&dir).unwrap();
        record::write(
            &dir,
            &SessionRecord {
                id: id.to_string(),
                agent: agent.to_string(),
                backend: Backend::HarnessAdi,
                cwd: PathBuf::from("/tmp"),
                message: message.to_string(),
                started_at: record::started_at(id),
                last_activity: 0,
                hidden: false,
                runner_state: None,
            },
        )
        .unwrap();
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

        // Sessions of another agent are another agent's.
        assert!(store.list("other").is_empty());
        assert!(store.get("other", &first.id).is_none());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The bug the flat layout exists to fix: two sessions of one agent run by *different* engines
    /// sit side by side, and nothing about the backend decides where they are found. Re-pointing the
    /// agent at another engine cannot make either disappear, because the path never mentioned one.
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

        // And the layout itself: one directory per agent, no executor segment anywhere in it.
        let dir = store.dir().join("solver");
        assert!(record::meta_path(&dir, &harness.id).is_file());
        assert!(!store.dir().join("harness").exists());
        assert!(!store.dir().join("process").exists());
        assert_eq!(
            store.log_path("solver", &harness.id),
            dir.join(format!("{}.log", harness.id)),
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Hiding is a flag and nothing else: the session stays in the history with its task intact, the
    /// flag survives a re-read (which is what makes it survive a reload), and writing it never reads
    /// as the session having just moved.
    #[test]
    fn hiding_a_session_only_flags_it() {
        let store = scratch("hidden");
        let now = now_ms();
        let id = format!("{:013}-0001", now - 3_600_000);
        seed(&store, "talker", &id, "some task");
        let dir = store.agent_dir("talker");
        std::fs::write(record::log_path(&dir, &id), "hello").unwrap();
        // Its files were written just now, so back-date them to when it was actually talking.
        let hour_ago = SystemTime::now() - Duration::from_secs(3_600);
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            File::options()
                .write(true)
                .open(entry.path())
                .unwrap()
                .set_modified(hour_ago)
                .unwrap();
        }

        let before = store.get("talker", &id).expect("listed");
        assert!(!before.hidden, "a session nobody hid is not hidden");
        assert!(before.last_activity < now - 60_000, "the files really are old");

        assert!(
            store.set_hidden("talker", &id, true).expect("hide"),
            "an existing session is there to flag",
        );
        let hidden = store.get("talker", &id).expect("still listed");
        assert!(hidden.hidden, "the flag round-trips through the record");
        assert_eq!(hidden.message, before.message, "the task is not lost");
        assert_eq!(hidden.started_at, before.started_at);
        assert_eq!(
            hidden.last_activity, before.last_activity,
            "hiding is not activity — the record's mtime is left out of it",
        );
        assert_eq!(store.list("talker").len(), 1, "and it is still listed");

        // Unhiding is the same write in reverse, and an unknown session is a quiet no-op.
        assert!(store.set_hidden("talker", &id, false).expect("unhide"));
        assert!(!store.get("talker", &id).expect("listed").hidden);
        assert!(
            !store.set_hidden("talker", "0000000000001-0000", true).expect("absent"),
            "a session that isn't there is nothing to flag",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Activity follows the files, not the clock the session started on — that is what makes a long,
    /// quiet conversation sort below one answered a minute ago — and it never precedes the start.
    #[test]
    fn activity_follows_the_files_but_never_precedes_the_start() {
        let store = scratch("activity");
        let now = now_ms();
        let dir = store.agent_dir("talker");

        // Started an hour ago, wrote just now: it must read as active now, not an hour back.
        let quiet = format!("{:013}-0001", now - 3_600_000);
        seed(&store, "talker", &quiet, "old but busy");
        std::fs::write(record::log_path(&dir, &quiet), "hello").unwrap();
        std::fs::write(dir.join(format!("{quiet}.jsonl")), "{}\n").unwrap();

        // An id claiming to start in an hour: its files are already older than that.
        let future = format!("{:013}-0002", now + 3_600_000);
        seed(&store, "talker", &future, "from the future");
        std::fs::write(record::log_path(&dir, &future), "hello").unwrap();

        let busy = store.get("talker", &quiet).expect("listed");
        assert_eq!(busy.started_at, now - 3_600_000, "start comes from the id");
        assert!(
            busy.last_activity >= now - 60_000,
            "activity comes from the files, not the id: {} vs {now}",
            busy.last_activity,
        );

        let odd = store.get("talker", &future).expect("listed");
        assert_eq!(
            odd.last_activity, odd.started_at,
            "an mtime older than the start never drags activity backwards",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Deleting takes *everything* a session owns — including a sidecar this module has never heard
    /// of, which is the whole reason the sweep goes by prefix — and nothing of its neighbour's.
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
            std::fs::write(dir.join(format!("{id}.transcript.jsonl")), "{}\n").unwrap();
        }
        store.enqueue("chat", &doomed.id, "waiting").expect("enqueue");

        assert!(store.delete("chat", &doomed.id).expect("delete"));
        for path in [
            record::meta_path(&dir, &doomed.id),
            record::log_path(&dir, &doomed.id),
            queue::path(&dir, &doomed.id),
            dir.join(format!("{}.transcript.jsonl", doomed.id)),
        ] {
            assert!(!path.exists(), "{} survived the delete", path.display());
        }
        assert!(store.get("chat", &doomed.id).is_none());

        // …and only its own.
        let left = store.list("chat");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, keeper.id);
        assert!(dir.join(format!("{}.transcript.jsonl", keeper.id)).exists());

        // Deleting what is already gone is quiet, so a second click is not an error.
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

        // Nothing over the cap sweeps nothing.
        let store_small = scratch("prune-small");
        seed(&store_small, "busy", "0000000000001-0000", "task");
        assert_eq!(store_small.prune_old("busy", |_| false), 0);
        let _ = std::fs::remove_dir_all(store_small.dir());

        // The oldest of the excess is still running, so it stays and one fewer is swept.
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

        // Once it finishes, the next prune takes it too.
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

        assert_eq!(store.enqueue("chat", &a.id, "one").expect("enqueue"), 1);
        assert_eq!(store.enqueue("chat", &a.id, "two").expect("enqueue"), 2);
        assert_eq!(store.enqueue("chat", &b.id, "elsewhere").expect("enqueue"), 1);
        assert_eq!(store.queue_len("chat", &a.id), 2);
        assert_eq!(store.queued("chat", &a.id), ["one", "two"]);
        assert_eq!(
            store.queued("chat", &b.id),
            ["elsewhere"],
            "one session's queue is not another's",
        );

        assert_eq!(
            store.dequeue("chat", &a.id).expect("dequeue").as_deref(),
            Some("one"),
        );
        assert_eq!(store.queued("chat", &a.id), ["two"]);

        assert_eq!(store.enqueue("chat", &a.id, "three").expect("enqueue"), 2);
        assert!(store.unqueue("chat", &a.id, 0).expect("unqueue"));
        assert_eq!(store.queued("chat", &a.id), ["three"]);
        assert!(!store.unqueue("chat", &a.id, 9).expect("past the end"));

        store.clear_queue("chat", &a.id).expect("clear");
        assert_eq!(store.queue_len("chat", &a.id), 0);
        assert_eq!(
            store.queued("chat", &b.id),
            ["elsewhere"],
            "clearing one queue leaves the other",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The store owns a session's turns, so a reader asks it — not a runner — for the conversation:
    /// what was recorded, what is being said now, and what is still waiting. Only the first of those
    /// is on disk.
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
        // Mid-turn: the answer streams into the view, and two more messages wait behind it.
        let live = TurnContent {
            text: "looking".into(),
            steps: vec![crate::progress::Step::Message {
                text: "one moment".into(),
            }],
            metrics: None,
        };
        store.enqueue("chat", id, "and restart it").expect("enqueue");
        store.enqueue("chat", id, "then tell me").expect("enqueue");

        let view = store.transcript("chat", id, Some(live.clone()), true);
        assert_eq!(view.len(), 4);
        assert!(view[1].pending, "the answer is still being written");
        assert_eq!(view[1].text, "looking");
        assert_eq!(
            view[2..].iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["and restart it", "then tell me"],
        );
        assert!(view[2..].iter().all(|t| t.queued));
        assert_eq!(store.turns("chat", id).len(), 1, "none of that is on disk");

        // The turn lands: committing it is what makes it durable, and the same live content handed
        // in afterwards is no longer spliced on top of it.
        store
            .append_turn("chat", id, assistant_turn(&live))
            .expect("append");
        let settled = store.transcript("chat", id, Some(live), false);
        assert_eq!(settled.len(), 4, "the answer, once, then the queue");
        assert!(!settled[1].pending);
        assert_eq!(store.turns("chat", id).len(), 2);

        // One session's transcript is not another's, and a session that is gone cannot be written to.
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
        assert!(
            !transcript::path(&store.agent_dir("chat"), id).exists(),
            "and the transcript went with the delete",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A session whose record was lost is still reachable — it has output, and output is the part
    /// nobody can regenerate. Its start comes back from its id and everything else reads as unset.
    #[test]
    fn a_session_with_only_a_log_is_still_listed() {
        let store = scratch("orphan");
        let dir = store.agent_dir("solver");
        std::fs::create_dir_all(&dir).unwrap();
        let id = "0000000054321-0000";
        std::fs::write(record::log_path(&dir, id), "output nobody recorded").unwrap();

        let listed = store.list("solver");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].started_at, 54_321, "recovered from the id");
        assert_eq!(listed[0].message, "");
        assert!(store.get("solver", id).is_some());
        // And it can still be hidden and deleted, which mints the record it never had.
        assert!(store.set_hidden("solver", id, true).expect("hide"));
        assert!(store.get("solver", id).expect("listed").hidden);
        assert!(store.delete("solver", id).expect("delete"));
        assert!(store.list("solver").is_empty());

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
