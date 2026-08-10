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
    /// How this session ended, written once the first time anybody notices it stopped running.
    ///
    /// `None` means either still running or ended before this column existed. Kept on the record
    /// rather than re-read from the log, because "did it work?" is the question every listing is
    /// really asking and the log is a megabyte of somebody else's wire format.
    pub outcome: Option<RunOutcome>,
}

/// What became of a run: the engine's own verdict, its telemetry, and the head of what it said.
///
/// Stored as JSON in one column rather than as columns of its own. The fields are an engine's
/// self-report and differ between them — the Claude CLI's `terminal_reason` has no counterpart in
/// the adi loop — so a column apiece would be a table mostly full of nulls, migrated again every
/// time an engine learns to say something new.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunOutcome {
    /// The engine's own word for how it ended (`completed`, `api_error`, `aborted_tools`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    /// Whether the engine called it a failure.
    #[serde(default)]
    pub is_error: bool,
    /// Cost in micro-dollars (1e-6 USD), as the engine reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
    /// The opening of the run's answer — enough to tell a run that did the work from one that
    /// died saying it was not logged in, without opening anything.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_head: String,
    /// Unix milliseconds this was recorded — when the ending was *noticed*, which for a run
    /// nobody watched is later than when it happened.
    #[serde(default)]
    pub noted_at: u64,
}

/// How much of the answer travels with the outcome.
const MAX_RESULT_HEAD: usize = 400;

impl RunOutcome {
    /// Whether the engine said anything at all about how this ended.
    ///
    /// A run whose log held no telemetry to parse still gets an outcome — that it stopped, and
    /// when it was noticed — and that record must not be read as *it went fine*. `false` here is
    /// "nobody knows", which is the one case worth opening the log for; a caller that flattens it
    /// to a word should say so rather than pick the optimistic one.
    #[must_use]
    pub fn is_reported(&self) -> bool {
        self.terminal_reason.is_some()
            || self.is_error
            || self.duration_ms.is_some()
            || self.cost_micro_usd.is_some()
            || self.num_turns.is_some()
    }

    /// The outcome of a finished run, from whatever its engine reported and what it said.
    #[must_use]
    pub fn of(metrics: Option<&crate::progress::TurnMetrics>, answer: &str, noted_at: u64) -> Self {
        let mut outcome = Self {
            result_head: head(answer),
            noted_at,
            ..Self::default()
        };
        if let Some(m) = metrics {
            outcome.terminal_reason = m.terminal_reason.clone();
            outcome.is_error = m.is_error;
            outcome.cost_micro_usd = m.cost_micro_usd;
            outcome.duration_ms = m.duration_ms;
            outcome.num_turns = m.num_turns;
        }
        outcome
    }
}

/// `text`'s first [`MAX_RESULT_HEAD`] characters, cut on a character boundary and marked when cut.
fn head(text: &str) -> String {
    let text = text.trim();
    let mut out: String = text.chars().take(MAX_RESULT_HEAD).collect();
    if out.chars().count() < text.chars().count() {
        out.push('…');
    }
    out
}

/// Build a record from a row of the columns named by
/// [`RECORD_COLUMNS`](super::RECORD_COLUMNS), in that order.
pub(super) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let state: Option<String> = row.get(8)?;
    let outcome: Option<String> = row.get(9)?;
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
        // Same rule, and it matters more here: an outcome written by a newer build must not make
        // an older one unable to list the session at all.
        outcome: outcome.and_then(|t| serde_json::from_str(&t).ok()),
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
