//! What the store knows about a session: one row of `sessions`, and the id it is filed under.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Backend;

/// The suffix of a session's raw output log — the one thing about a session that is still a file.
pub(super) const LOG_SUFFIX: &str = "log";

/// Everything durable about a session that is not the process running it.
///
/// The `backend` is a column rather than a place. That is the whole point of the layout: an executor
/// used to be a directory level, so changing this one field moved where the history was looked for
/// and the old runs became unreachable. As a value it is just a label on a session that stays
/// exactly where it was filed.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The session id — minted at creation, and the name every file it owns is prefixed with.
    pub id: String,
    /// The agent this session belongs to.
    pub agent: String,
    /// Which engine ran it — a label carried with the session, not a place it is kept.
    pub backend: Backend,
    /// The directory this session runs in, resolved once at creation and re-used for every later
    /// turn. An engine's own session store is keyed by the directory it ran in, so re-resolving
    /// mid-conversation makes the session unresumable and puts earlier turns' files out of reach.
    pub cwd: PathBuf,
    /// What the session was opened with — its title in any listing.
    pub message: String,
    /// Unix milliseconds the session began. Also encoded in the id.
    pub started_at: u64,
    /// Unix milliseconds the session last *said* something: the moment on the last turn recorded
    /// for it, never earlier than its start.
    ///
    /// Maintained by [`append_turn`](super::SessionStore::append_turn) and by nothing else, which is
    /// the whole of its meaning. It used to be derived per read from the newest mtime across the
    /// session's files, which read as "the last moment it moved" — and moving is not speaking. A run
    /// spooling into its log, a runner parking a sidecar, and above all a finished answer being
    /// committed by the act of *opening the chat* all touched a file, and each one sent a
    /// conversation whose last word was said days ago back to the top of a rail sorted by this.
    pub last_activity: u64,
    /// Whether a reader has hidden this session from the rail. A listing preference and nothing
    /// more — a hidden session still runs, still keeps its log, and is still returned here; it is
    /// the *view* that leaves it out.
    pub hidden: bool,
    /// The runner's own scratch space — runner-written, store-opaque, never interpreted here.
    ///
    /// A local-process runner keeps its pid in it, a pty runner its session name, and a runner whose
    /// engine lives behind an API keeps nothing at all. That last case is why this is an untyped
    /// slot rather than a handle the store would have to understand.
    pub runner_state: Option<serde_json::Value>,
}

/// Build a record from a row of the columns named by
/// [`RECORD_COLUMNS`](super::RECORD_COLUMNS), in that order.
pub(super) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let state: Option<String> = row.get(8)?;
    Ok(SessionRecord {
        agent: row.get(0)?,
        id: row.get(1)?,
        backend: Backend::from(row.get::<_, String>(2)?.as_str()),
        cwd: PathBuf::from(row.get::<_, String>(3)?),
        message: row.get(4)?,
        started_at: row.get(5)?,
        last_activity: row.get(6)?,
        hidden: row.get::<_, i64>(7)? != 0,
        // A slot that will not parse reads as empty rather than failing the row: it belongs to a
        // runner, and a listing has no stake in what is in it.
        runner_state: state.and_then(|t| serde_json::from_str(&t).ok()),
    })
}

/// Disambiguates ids minted within the same millisecond by one process.
static ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique, time-sortable session id: `<unix_millis>-<seq>`.
///
/// The millis prefix is zero-padded so ids sort lexicographically by start time — which is what lets
/// the listing's index answer "newest first" without a sort — and the sequence separates two
/// sessions opened in the same millisecond.
pub(super) fn new_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{ms:013}-{seq:04}")
}

/// The unix-millis start time encoded in a session id, or 0 if it can't be parsed.
pub(super) fn started_at(id: &str) -> u64 {
    id.split_once('-')
        .and_then(|(ms, _)| ms.parse().ok())
        .unwrap_or(0)
}

pub(super) fn log_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{LOG_SUFFIX}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The id is not just unique, it is *ordered* — and it carries its own start time.
    #[test]
    fn an_id_sorts_by_and_remembers_when_it_was_minted() {
        let first = new_id();
        let second = new_id();
        assert_ne!(first, second);
        assert!(first < second, "{first} should sort before {second}");
        assert!(
            started_at(&first) > 1_600_000_000_000,
            "an id decodes to a plausible unix-millis time: {first}",
        );
        assert_eq!(started_at("not-an-id"), 0);
        assert_eq!(started_at("nodashesatall"), 0);
    }
}
