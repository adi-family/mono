//! The store's answer to [`Session`]: a borrowed view that reads from disk on every call.
//!
//! This owns **no session state**, and that is load-bearing rather than tidiness. A run can be
//! started by the CLI, advanced by the daemon, and watched in the app — three processes, none of
//! which can see another's memory. Anything cached here would be a second truth that one of the
//! others has already invalidated: a `has_started` read once would keep answering `false` after
//! another process ran the first turn, and a runner would establish a second engine session on top
//! of a live one.
//!
//! So a view is three strings and a borrow, and every question it answers is a fresh read.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::runner::{Session, StateWriter};

use super::{SessionRecord, SessionStore};

/// A borrowed view of one session in one store.
///
/// Cheap to make and meant to be made freely — take one per call rather than holding it, since it
/// borrows the store and pins nothing else.
#[derive(Debug, Clone)]
pub struct SessionRef<'a> {
    store: &'a SessionStore,
    agent: String,
    id: String,
    /// The log's location, resolved once.
    ///
    /// Not cached state: it is a pure function of the three fields above, none of which change for
    /// the life of the view, so there is nothing here that another process could invalidate. It is
    /// a field only because [`Session::log_path`] hands out a `&Path` — deliberately, since a
    /// spawned child needs a real file descriptor to redirect into — and a borrow has to point at
    /// something that outlives the call.
    log: PathBuf,
    /// Where [`Session::state`] is answered from.
    state: StateSource,
}

/// Whether this view asks for the runner's state slot, or already has the copy a listing read.
#[derive(Debug, Clone)]
enum StateSource {
    /// Ask, every time. The default, and the only thing a writer may use.
    Fresh,
    /// The value the record was listed with — see
    /// [`session_as_listed`](SessionStore::session_as_listed).
    Listed(Option<serde_json::Value>),
}

impl<'a> SessionRef<'a> {
    pub(super) fn new(store: &'a SessionStore, agent: &str, id: &str) -> Self {
        Self {
            store,
            agent: agent.to_string(),
            id: id.to_string(),
            log: store.log_path(agent, id),
            state: StateSource::Fresh,
        }
    }

    /// A read-only view whose state slot is already in hand.
    pub(super) fn as_listed(
        store: &'a SessionStore,
        agent: &str,
        id: &str,
        state: Option<serde_json::Value>,
    ) -> Self {
        Self {
            state: StateSource::Listed(state),
            ..Self::new(store, agent, id)
        }
    }

    /// The store this view reads through — for a caller holding a view that needs the record, the
    /// queue, or anything else the [`Session`] trait deliberately leaves out.
    #[must_use]
    pub fn store(&self) -> &'a SessionStore {
        self.store
    }

    /// This session's record as it is on disk right now, or `None` if it has been deleted.
    #[must_use]
    pub fn record(&self) -> Option<SessionRecord> {
        self.store.get(&self.agent, &self.id)
    }
}

impl Session for SessionRef<'_> {
    fn id(&self) -> &str {
        &self.id
    }

    fn agent(&self) -> &str {
        &self.agent
    }

    /// Answered by the log: the store creates a record, a runner creates the log when it first
    /// spawns into it, so a log that exists means a turn has been started here.
    ///
    /// Deliberately not answered by the [state slot](Session::state) — a runner whose engine lives
    /// behind an API writes nothing there, and its second turn must still resume rather than open a
    /// new conversation.
    fn has_started(&self) -> bool {
        self.log.exists()
    }

    /// Fresh unless this view was built from a listed record, which already carried it.
    fn state(&self) -> Option<serde_json::Value> {
        match &self.state {
            StateSource::Fresh => self.store.runner_state(&self.agent, &self.id),
            StateSource::Listed(state) => state.clone(),
        }
    }

    fn set_state(&self, value: serde_json::Value) -> Result<()> {
        self.store.set_runner_state(&self.agent, &self.id, value)
    }

    /// The store is a path and nothing else, so the owned half of this view is the same three
    /// fields with the borrow turned into a clone — no cached state travels with it, and it reads
    /// through to the same files.
    fn state_writer(&self) -> Option<Box<dyn StateWriter>> {
        Some(Box::new(StoredState {
            store: self.store.clone(),
            agent: self.agent.clone(),
            id: self.id.clone(),
        }))
    }

    fn log_path(&self) -> &Path {
        &self.log
    }
}

/// [`SessionRef`]'s owned counterpart: the same session, reached without borrowing the store.
#[derive(Debug)]
struct StoredState {
    store: SessionStore,
    agent: String,
    id: String,
}

impl StateWriter for StoredState {
    /// Always fresh. This is the *writing* half: a runner that decides what to write from a
    /// snapshot decides it from a value another process may already have moved past.
    fn state(&self) -> Option<serde_json::Value> {
        self.store.runner_state(&self.agent, &self.id)
    }

    fn set_state(&self, value: serde_json::Value) -> Result<()> {
        self.store.set_runner_state(&self.agent, &self.id, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-store-session-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    /// The point of the whole type: a view held across a change made *elsewhere* must see that
    /// change. Here "elsewhere" is a second view over the same store, which is as close as a test
    /// gets to the daemon writing while the app reads.
    #[test]
    fn a_view_sees_what_another_writer_did_after_it_was_taken() {
        let store = scratch("fresh");
        let record = store
            .create("solver", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");
        let view = store.session("solver", &record.id);
        assert_eq!(view.id(), record.id);
        assert_eq!(view.agent(), "solver");
        assert!(!view.has_started(), "nothing has run in it yet");
        assert!(view.state().is_none(), "no runner has been here");

        let elsewhere = store.session("solver", &record.id);
        elsewhere
            .set_state(serde_json::json!({ "pid": 4711 }))
            .expect("set state");
        std::fs::write(view.log_path(), "first turn").expect("the runner opens the log");

        assert!(view.has_started(), "the view re-reads rather than remembers");
        assert_eq!(
            view.state(),
            Some(serde_json::json!({ "pid": 4711 })),
            "and it reads the other writer's state slot, not a stale copy",
        );
        view.set_state(serde_json::json!({ "session": "adi-agent-solver" }))
            .expect("replace state");
        let after = view.record().expect("still there");
        assert_eq!(
            after.runner_state,
            Some(serde_json::json!({ "session": "adi-agent-solver" })),
        );
        assert_eq!(after.message, "go");
        assert_eq!(after.backend, Backend::HarnessAdi);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A runner writing into a session that has been deleted must be told, not quietly resurrect it
    /// as a record with nothing but a state slot in it.
    #[test]
    fn writing_state_into_a_deleted_session_fails_rather_than_recreating_it() {
        let store = scratch("deleted");
        let record = store
            .create("solver", Backend::ProcessClaude, "/tmp", "go")
            .expect("create");
        let view = store.session("solver", &record.id);
        assert!(store.delete("solver", &record.id).expect("delete"));

        assert!(view.record().is_none());
        assert!(view.state().is_none());
        assert!(
            view.set_state(serde_json::json!({ "pid": 1 })).is_err(),
            "the session is gone; writing to it is an error",
        );
        assert!(store.get("solver", &record.id).is_none(), "still gone");

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
