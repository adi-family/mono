//! What the store knows about a session, and the sidecar it keeps it in.
//!
//! One JSON file per session, `<id>.meta.json`, beside the session's other files. It is written
//! whole every time — there is no partial update — so a sidecar that predates a field simply reads
//! as that field's default rather than needing a migration.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Backend;
use crate::error::{Error, Result};

/// The suffix of a session's metadata sidecar, as it reads *after* the session id.
///
/// Every file a session owns is `<id>.<something>` — `.log`, `.queue.json`, and whatever a runner
/// spools beside them — which is what lets one prefix sweep take all of them without a list of
/// extensions that a new sidecar would silently fall off the end of.
pub(super) const META_SUFFIX: &str = "meta.json";

/// The suffix of a session's raw output log.
pub(super) const LOG_SUFFIX: &str = "log";

/// Everything durable about a session that is not the process running it.
///
/// The `backend` is here, in the record, rather than in the path. That is the whole point of the
/// layout: an executor used to be a directory level, so changing this one field moved where the
/// history was looked for and the old runs became unreachable. As a field it is just a label on a
/// session that stays exactly where it was filed.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The session id — minted at creation, and the name every file it owns is prefixed with.
    ///
    /// Written to the sidecar for a human reading it, never read back from there: the file's own
    /// name is the id, and a copied or renamed sidecar must read as the session it now *is* rather
    /// than the one it was taken from.
    #[serde(skip_deserializing)]
    pub id: String,
    /// The agent this session belongs to. Recovered from the directory, for the same reason as
    /// [`id`](Self::id).
    #[serde(skip_deserializing)]
    pub agent: String,
    /// Which engine ran it — a label carried with the session, not a place it is kept.
    #[serde(default)]
    pub backend: Backend,
    /// The directory this session runs in, resolved once at creation and re-used for every later
    /// turn. An engine's own session store is keyed by the directory it ran in, so re-resolving
    /// mid-conversation makes the session unresumable and puts earlier turns' files out of reach.
    #[serde(default)]
    pub cwd: PathBuf,
    /// What the session was opened with — its title in any listing.
    #[serde(default)]
    pub message: String,
    /// Unix milliseconds the session began. Also encoded in the id, which is where it is recovered
    /// from when a sidecar is missing or predates the field.
    #[serde(default)]
    pub started_at: u64,
    /// Unix milliseconds the session last did anything.
    ///
    /// Derived on every read from the mtimes of the files it owns, never stored: a written copy
    /// would be stale the moment the log grew, and a second truth about the same fact is exactly
    /// what this refactor is removing.
    #[serde(skip)]
    pub last_activity: u64,
    /// Whether a reader has hidden this session from the rail. A listing preference and nothing
    /// more — a hidden session still runs, still keeps its log, and is still returned here; it is
    /// the *view* that leaves it out.
    #[serde(default)]
    pub hidden: bool,
    /// The runner's own scratch space — runner-written, store-opaque, never interpreted here.
    ///
    /// A local-process runner keeps its pid in it, a pty runner its session name, and a runner whose
    /// engine lives behind an API keeps nothing at all. That last case is why this is an untyped
    /// slot rather than a handle the store would have to understand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_state: Option<serde_json::Value>,
}

/// Disambiguates ids minted within the same millisecond by one process.
static ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique, time-sortable session id: `<unix_millis>-<seq>`.
///
/// The millis prefix is zero-padded so ids sort lexicographically by start time — which is what
/// makes "newest first" a sort of the file names, with no sidecar read per candidate — and the
/// sequence separates two sessions opened in the same millisecond.
pub(super) fn new_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{ms:013}-{seq:04}")
}

/// The unix-millis start time encoded in a session id, or 0 if it can't be parsed.
///
/// The fallback for a session whose sidecar is gone: its id still says when it began, so a listing
/// shows a real moment rather than the epoch.
pub(super) fn started_at(id: &str) -> u64 {
    id.split_once('-')
        .and_then(|(ms, _)| ms.parse().ok())
        .unwrap_or(0)
}

pub(super) fn meta_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{META_SUFFIX}"))
}

pub(super) fn log_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{LOG_SUFFIX}"))
}

/// The record filed under `<dir>/<id>`, all-default when the sidecar is absent or unreadable.
///
/// Tolerant on purpose: a session whose sidecar was lost still has a log worth listing, and it reads
/// as never hidden, with no recorded task, and its start recovered from its id. `last_activity` is
/// left at zero — only the store, which is already walking the directory, can fill it in.
pub(super) fn read(dir: &Path, agent: &str, id: &str) -> SessionRecord {
    let mut record = std::fs::read_to_string(meta_path(dir, id))
        .ok()
        .and_then(|text| serde_json::from_str::<SessionRecord>(&text).ok())
        .unwrap_or_default();
    record.id = id.to_string();
    record.agent = agent.to_string();
    if record.started_at == 0 {
        record.started_at = started_at(id);
    }
    record
}

/// Write the record back, whole.
///
/// # Errors
/// Returns the write error. Unlike the queue, a record that fails to persist is worth failing over:
/// it is the only thing that says the session exists.
pub(super) fn write(dir: &Path, record: &SessionRecord) -> Result<()> {
    let text = serde_json::to_string(record)
        .map_err(|e| Error::Session(format!("couldn't encode session {}: {e}", record.id)))?;
    std::fs::write(meta_path(dir, &record.id), text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The id is not just unique, it is *ordered* — and it carries its own start time, which is what
    /// a session with no sidecar left is listed by.
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

    /// The sidecar carries the backend as a value. This is the check that it round-trips as the
    /// wire string an agent manifest uses, so a record written by one version reads back the same
    /// engine in the next.
    #[test]
    fn a_record_round_trips_through_its_sidecar() {
        let record = SessionRecord {
            id: "0000000000001-0000".into(),
            agent: "solver".into(),
            backend: Backend::HarnessAdi,
            cwd: PathBuf::from("/tmp/work"),
            message: "do the thing".into(),
            started_at: 1,
            last_activity: 99,
            hidden: true,
            runner_state: Some(serde_json::json!({ "pid": 4711 })),
        };
        let text = serde_json::to_string(&record).expect("encode");
        assert!(
            text.contains(r#""backend":"harness:adi""#),
            "the backend is a plain field: {text}",
        );
        assert!(
            !text.contains("last_activity"),
            "derived activity is never written down: {text}",
        );

        let mut back: SessionRecord = serde_json::from_str(&text).expect("decode");
        assert_eq!(back.id, "", "identity comes from the path, not the file");
        assert_eq!(back.agent, "");
        assert_eq!(back.last_activity, 0);
        back.id = record.id.clone();
        back.agent = record.agent.clone();
        back.last_activity = record.last_activity;
        assert_eq!(back, record);
    }

    /// A sidecar that is missing, truncated, or from before a field existed must not take a session
    /// out of the listing — its id alone still says who it is and when it started.
    #[test]
    fn an_unreadable_sidecar_reads_as_defaults_with_the_start_from_the_id() {
        let dir = std::env::temp_dir().join(format!(
            "adi-store-record-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let id = "0000000012345-0007";
        let absent = read(&dir, "solver", id);
        assert_eq!(absent.id, id);
        assert_eq!(absent.agent, "solver");
        assert_eq!(absent.started_at, 12_345, "recovered from the id");
        assert!(!absent.hidden);
        assert_eq!(absent.backend, Backend::default());

        std::fs::write(meta_path(&dir, id), "{ not json").unwrap();
        assert_eq!(read(&dir, "solver", id).started_at, 12_345);

        // A sidecar from before `cwd` and `runner_state` existed still reads.
        std::fs::write(
            meta_path(&dir, id),
            r#"{"message":"old one","started_at":42}"#,
        )
        .unwrap();
        let old = read(&dir, "solver", id);
        assert_eq!(old.message, "old one");
        assert_eq!(old.started_at, 42, "a recorded start wins over the id");
        assert_eq!(old.cwd, PathBuf::new());
        assert!(old.runner_state.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
