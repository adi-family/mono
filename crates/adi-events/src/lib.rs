//! adi-events — a tiny, decoupled event bus for the adi platform.
//!
//! A **publisher** calls [`Events::emit`] with a dotted event name (`adi.tasks.created`) and a
//! JSON payload; the event is written as one small record file into a spool directory
//! (`~/.adi/mono/events`). A **consumer** — the app's event dispatcher — calls [`Events::drain`]
//! to read every spooled event, delivers each to whoever subscribed, and [`Events::remove`]s it.
//!
//! The point is decoupling *across processes*: a publisher (the task store, an agent run, the
//! CLI) depends only on this crate, which depends only on [`adi_config`]. It never learns that a
//! subscriber exists — it just drops a record. Whatever consumes the spool is free to change
//! without touching a single publisher, exactly like the rest of the platform's file-backed,
//! poll-to-reconcile design (trigger manifests, run states).
//!
//! Event names are dotted **topics**; subscribers match them with [`matches`], a segment-aware
//! glob where `*` matches exactly one segment and `**` matches the remaining tail:
//!
//! ```
//! use adi_events::matches;
//! assert!(matches("adi.tasks.*", "adi.tasks.created"));
//! assert!(!matches("adi.tasks.*", "adi.tasks.sub.created"));
//! assert!(matches("adi.tasks.**", "adi.tasks.sub.created"));
//! assert!(matches("adi.**", "adi.agents.run.started"));
//! ```
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-events-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_events::Events;
//!
//! # let bus = Events::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let bus = Events::open();
//! bus.emit("adi.tasks.created", r#"{"id":"t1"}"#)?;
//!
//! let spooled = bus.drain()?;
//! assert_eq!(spooled.len(), 1);
//! assert_eq!(spooled[0].record.name, "adi.tasks.created");
//! bus.remove(&spooled[0].path)?;
//! assert!(bus.drain()?.is_empty());
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_events::Error>(())
//! ```

mod catalog;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use adi_config::{Config, now_unix};
use serde::{Deserialize, Serialize};

pub use catalog::{ENVELOPE, EventType};

/// The result type every fallible `adi-events` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong emitting or draining events.
#[derive(Debug)]
pub enum Error {
    /// An event name is empty or carries a character outside `[A-Za-z0-9._-]`.
    InvalidName(String),
    /// The underlying config store failed (writing or removing a record file).
    Config(adi_config::Error),
    /// A directory operation (listing the spool) failed.
    Io(std::io::Error),
    /// A record couldn't be encoded to JSON.
    Encode(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(name) => write!(
                f,
                "invalid event name {name:?}: use dotted segments of letters, digits, '.', '-', '_'"
            ),
            Self::Config(e) => write!(f, "event store error: {e}"),
            Self::Io(e) => write!(f, "event store I/O error: {e}"),
            Self::Encode(e) => write!(f, "event encode error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Encode(e) => Some(e),
            Self::InvalidName(_) => None,
        }
    }
}

impl From<adi_config::Error> for Error {
    fn from(e: adi_config::Error) -> Self {
        Self::Config(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// The store module the spool lives under, and each record file's extension.
const MODULE: &str = "events";
const RECORD_EXT: &str = "json";

/// Cap on spooled records. If nothing consumes the spool (the app is down), publishers would
/// otherwise fill the disk; once the newest [`MAX_SPOOL`] are on disk, [`emit`](Events::emit)
/// prunes the oldest so growth is bounded and a burst of stale events can't ambush the consumer
/// when it next comes up.
const MAX_SPOOL: usize = 2000;

/// Per-process counter so two events emitted in the same second get distinct, ordered filenames.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// One published event: a dotted topic [`name`](Self::name), a JSON [`payload`](Self::payload)
/// string (opaque to the bus — the subscriber parses it), and when it was emitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    /// The dotted event topic, e.g. `adi.tasks.created`.
    pub name: String,
    /// The event body, as a string — JSON by convention, but the bus never parses it.
    #[serde(default)]
    pub payload: String,
    /// When the event was emitted, as Unix epoch seconds.
    #[serde(default)]
    pub emitted_at: u64,
}

/// A record as it sits in the spool: the file backing it (pass to [`Events::remove`] once
/// delivered) and the parsed [`EventRecord`].
#[derive(Debug, Clone)]
pub struct SpooledEvent {
    /// The record file on disk.
    pub path: PathBuf,
    /// The parsed event.
    pub record: EventRecord,
}

/// The event bus: emits records into, and drains records out of, the `events` spool directory.
/// Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct Events {
    config: Config,
}

impl Default for Events {
    fn default() -> Self {
        Self::open()
    }
}

impl Events {
    /// Open the bus backed by the standard store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the bus backed by a caller-supplied [`Config`] — for tests, or to share the exact
    /// store another subsystem already holds (so a scratch store stays isolated).
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The store this bus reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The spool directory: `~/.adi/mono/events`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(MODULE).dir().to_path_buf()
    }

    /// Publish an event: write one record file into the spool, then prune the spool back under
    /// [`MAX_SPOOL`]. The write is atomic (temp-then-rename), so a draining consumer never reads
    /// a half-written record.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an empty or unsafe name, [`Error::Encode`] if the record won't
    /// serialize, or [`Error::Config`] on a write failure.
    pub fn emit(&self, name: &str, payload: impl Into<String>) -> Result<()> {
        validate_name(name)?;
        let record = EventRecord {
            name: name.to_string(),
            payload: payload.into(),
            emitted_at: now_unix(),
        };
        let bytes = serde_json::to_vec(&record).map_err(Error::Encode)?;
        let file = format!("{}.{RECORD_EXT}", record_stem(record.emitted_at));
        self.config.module(MODULE).write_raw(&file, &bytes)?;
        self.prune();
        Ok(())
    }

    /// Every spooled event, oldest first (filenames sort chronologically). A missing spool dir
    /// yields an empty list; an unparseable record is skipped.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure.
    pub fn drain(&self) -> Result<Vec<SpooledEvent>> {
        let mut spooled = self.spool_files()?;
        spooled.sort();
        Ok(spooled
            .into_iter()
            .filter_map(|path| {
                let bytes = std::fs::read(&path).ok()?;
                let record = serde_json::from_slice(&bytes).ok()?;
                Some(SpooledEvent { path, record })
            })
            .collect())
    }

    /// Remove a delivered record from the spool. A record already gone is not an error.
    ///
    /// # Errors
    /// [`Error::Io`] on a removal failure other than not-found.
    pub fn remove(&self, path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// The record files currently in the spool (full paths, unsorted). A missing dir is empty.
    fn spool_files(&self) -> Result<Vec<PathBuf>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        Ok(entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(RECORD_EXT))
            .collect())
    }

    /// Delete the oldest records until at most [`MAX_SPOOL`] remain. Best-effort: this guards a
    /// never-consumed spool from growing without bound, so a failure to prune must never fail the
    /// emit that triggered it.
    fn prune(&self) {
        let Ok(mut files) = self.spool_files() else {
            return;
        };
        if files.len() <= MAX_SPOOL {
            return;
        }
        files.sort();
        let excess = files.len() - MAX_SPOOL;
        for path in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The filename stem for a record emitted at `emitted_at`: the zero-padded second, this
/// process's pid, and a per-process counter. Sorts chronologically by the leading second, and is
/// unique across concurrent emitters (distinct pids) and within one process (the counter).
fn record_stem(emitted_at: u64) -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{emitted_at:010}-{:08}-{seq:08}", std::process::id())
}

/// Reject an event name that isn't a safe dotted topic: non-empty, every character a letter,
/// digit, `.`, `-`, or `_`. The rule matters because the name is matched against subscription
/// patterns and shows up in logs — not because it becomes a path (the record's filename is
/// generated, never the name itself). Public so a producer (or the catalog's coherence test) can
/// check a name against the same rule the bus enforces on [`Events::emit`].
///
/// # Errors
/// [`Error::InvalidName`] if `name` is empty or holds a character outside `[A-Za-z0-9._-]`.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(Error::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Whether a dotted event `subject` matches a subscription `pattern`, segment by segment:
///
/// * a literal segment matches itself exactly,
/// * `*` matches exactly one segment,
/// * `**` matches one or more remaining segments (a tail wildcard).
///
/// So `adi.tasks.*` matches `adi.tasks.created` but not `adi.tasks.sub.created`, while
/// `adi.tasks.**` matches both `adi.tasks.sub` and `adi.tasks.sub.created`. An exact pattern with
/// no wildcards is just equality.
#[must_use]
pub fn matches(pattern: &str, subject: &str) -> bool {
    let p: Vec<&str> = pattern.split('.').collect();
    let s: Vec<&str> = subject.split('.').collect();
    seg_match(&p, &s)
}

/// The recursive core of [`matches`], over the remaining pattern and subject segments.
fn seg_match(p: &[&str], s: &[&str]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            if p.len() == 1 {
                // A trailing `**` matches one or more remaining segments.
                !s.is_empty()
            } else {
                // `**` in the middle: consume one-or-more segments, then match the rest.
                (1..=s.len()).any(|k| seg_match(&p[1..], &s[k..]))
            }
        }
        (Some(&"*"), Some(_)) => seg_match(&p[1..], &s[1..]),
        (Some(pp), Some(ss)) if pp == ss => seg_match(&p[1..], &s[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Events {
        let root = std::env::temp_dir().join(format!(
            "adi-events-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Events::with_config(Config::with_root(root))
    }

    #[test]
    fn emit_then_drain_round_trips_and_remove_clears() {
        let bus = scratch("roundtrip");
        assert!(bus.drain().expect("empty drain").is_empty());

        bus.emit("adi.tasks.created", r#"{"id":"t1"}"#)
            .expect("emit");
        let spooled = bus.drain().expect("drain");
        assert_eq!(spooled.len(), 1);
        assert_eq!(spooled[0].record.name, "adi.tasks.created");
        assert_eq!(spooled[0].record.payload, r#"{"id":"t1"}"#);
        assert!(spooled[0].record.emitted_at > 0);

        bus.remove(&spooled[0].path).expect("remove");
        assert!(bus.drain().expect("drained empty").is_empty());
        // Removing an already-gone record is a no-op, not an error.
        bus.remove(&spooled[0].path).expect("idempotent remove");
    }

    #[test]
    fn drain_returns_events_in_emission_order() {
        let bus = scratch("order");
        for i in 0..5 {
            bus.emit("adi.tasks.created", format!("{{\"n\":{i}}}"))
                .expect("emit");
        }
        let names: Vec<String> = bus
            .drain()
            .expect("drain")
            .into_iter()
            .map(|s| s.record.payload)
            .collect();
        assert_eq!(
            names,
            vec![
                "{\"n\":0}",
                "{\"n\":1}",
                "{\"n\":2}",
                "{\"n\":3}",
                "{\"n\":4}"
            ]
        );
    }

    #[test]
    fn an_invalid_name_never_spools() {
        let bus = scratch("invalid");
        assert!(matches!(bus.emit("", "x"), Err(Error::InvalidName(_))));
        assert!(matches!(
            bus.emit("bad name", "x"),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            bus.emit("a/b", "x"),
            Err(Error::InvalidName(_))
        ));
        assert!(bus.drain().expect("still empty").is_empty());
    }

    #[test]
    fn segment_wildcard_matches_one_segment_only() {
        assert!(matches("adi.tasks.created", "adi.tasks.created"));
        assert!(!matches("adi.tasks.created", "adi.tasks.updated"));

        assert!(matches("adi.tasks.*", "adi.tasks.created"));
        assert!(matches("adi.tasks.*", "adi.tasks.updated"));
        assert!(!matches("adi.tasks.*", "adi.tasks.sub.created"));
        assert!(!matches("adi.tasks.*", "adi.tasks"));

        assert!(matches("*.tasks.created", "adi.tasks.created"));
        assert!(!matches("*.tasks.created", "adi.other.created"));
    }

    #[test]
    fn double_wildcard_matches_the_tail() {
        assert!(matches("adi.tasks.**", "adi.tasks.created"));
        assert!(matches("adi.tasks.**", "adi.tasks.sub.created"));
        // `**` requires at least one remaining segment.
        assert!(!matches("adi.tasks.**", "adi.tasks"));

        assert!(matches("adi.**", "adi.agents.run.started"));
        assert!(matches("**", "anything.at.all"));

        // `**` in the middle spans the gap.
        assert!(matches("adi.**.created", "adi.tasks.sub.created"));
        assert!(matches("adi.**.created", "adi.tasks.created"));
        assert!(!matches("adi.**.created", "adi.tasks.updated"));
    }

    #[test]
    fn prune_keeps_the_spool_bounded() {
        let bus = scratch("prune");
        for i in 0..(MAX_SPOOL + 25) {
            bus.emit("adi.load.tick", format!("{i}")).expect("emit");
        }
        assert_eq!(
            bus.drain().expect("drain").len(),
            MAX_SPOOL,
            "the spool is capped at MAX_SPOOL"
        );
    }
}
