//! adi-events — the adi platform's reliable event log.
//!
//! A **publisher** calls [`Events::emit_event`] with a typed [`Event`], or [`Events::emit`] with a
//! dotted event name (`adi.tasks.created`) and a JSON payload. The event lands as one row in an
//! append-only SQLite log (`~/.adi/mono/events/log.db`), under a monotonically increasing `seq`. A
//! **consumer** picks a stable subscriber name and calls [`Events::catch_up`] for everything past
//! its own [`Events::ack`]'d position — so a consumer that was never running, or crashed mid-batch,
//! replays exactly what it missed rather than losing it. Delivery is therefore at-least-once: a
//! consumer acks only once it has safely acted on a batch, and an event re-delivered after a crash
//! must be a no-op to act on twice, which is why every row also carries an `idempotency_key`
//! ([`Events::emit`] mints a fresh one per call; [`outbox::relay`] reuses the local row's own id, so
//! relaying the same pending row twice appends it only once).
//!
//! # The outbox half: writing in the same transaction as the state change
//!
//! This crate's own log lives in its own file, so a producer with *its own* database (`tasks.db`,
//! `sessions.db`, …) cannot insert into it as part of the producer's transaction — SQLite's
//! cross-database atomicity does not extend across separate files once either side is in WAL mode
//! (every store on this platform is). [`outbox`] is the other half of the pattern: a producer
//! concats [`outbox::SCHEMA`] into its own schema, calls [`outbox::write`] on its own transaction
//! right alongside the state change it describes — now genuinely atomic, because it is one write to
//! one file — and once that transaction commits, calls [`outbox::relay`] to forward the pending rows
//! into this crate's log. A relay that never runs (the process crashed right after commit) left the
//! event sitting in the local table rather than gone; the next relay of that same table — from this
//! process on its next write, or another process's — picks it up. See `adi-tasks`' `Tasks::with_doc`
//! for the concrete shape.
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
//! use adi_events::{Event, Events};
//!
//! #[derive(serde::Serialize)]
//! struct ItemCreated { id: String }
//! impl Event for ItemCreated {
//!     const NAME: &'static str = "my.items.created";
//! }
//!
//! # let bus = Events::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let bus = Events::open();
//! bus.emit_event(&ItemCreated { id: "item-1".into() })?;
//!
//! let caught_up = bus.catch_up("my-subscriber", 10)?;
//! assert_eq!(caught_up.len(), 1);
//! assert_eq!(caught_up[0].record.name, ItemCreated::NAME);
//! bus.ack("my-subscriber", caught_up[0].seq)?;
//! assert!(bus.catch_up("my-subscriber", 10)?.is_empty());
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_events::Error>(())
//! ```

mod catalog;
pub mod outbox;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use adi_config::{Config, now_unix};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub use catalog::{ENVELOPE, EventType};

/// The result type every fallible `adi-events` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong emitting, logging, or consuming events.
#[derive(Debug)]
pub enum Error {
    /// An event name is empty or carries a character outside `[A-Za-z0-9._-]`.
    InvalidName(String),
    /// The underlying config store failed (creating the module directory).
    Config(adi_config::Error),
    /// The log database rejected a statement, or a row couldn't be read back.
    Sqlite(rusqlite::Error),
    /// An event payload or record couldn't be encoded to JSON.
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
            Self::Sqlite(e) => write!(f, "event log error: {e}"),
            Self::Encode(e) => write!(f, "event encode error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Sqlite(e) => Some(e),
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

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

/// The store module the log lives under, and the database file within it.
const MODULE: &str = "events";
const DB_FILE: &str = "log.db";

/// Applied to every connection, in this order — `busy_timeout` must lead, since switching journal
/// mode takes a lock of its own and would otherwise fail outright against a store another process
/// is mid-write on (see `adi-db` and `adi-tasks`, which document the same trap).
const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma synchronous = NORMAL;\n";

/// The log's schema. `IF NOT EXISTS` throughout, so any process may be the one that creates it.
///
/// `idempotency_key` is unique so [`Events::append_record`] can treat a repeat append (an
/// [`outbox::relay`] retried after it crashed between inserting and deleting the local row) as the
/// no-op it must be, rather than a second row for the same fact.
///
/// `event_cursors` holds one row per subscriber that has ever called [`Events::catch_up`] or
/// [`Events::ack`]; a subscriber that has never shown up has no row, which is why
/// [`Events::retain`] treats "no cursors yet" as "nothing is safe to drop" rather than as an empty
/// minimum.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS events (
    seq             INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    idempotency_key TEXT    NOT NULL UNIQUE,
    name            TEXT    NOT NULL,
    payload         TEXT    NOT NULL,
    emitted_at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS event_cursors (
    subscriber TEXT    NOT NULL PRIMARY KEY,
    seq        INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0
);
";

/// Per-process counter so two events emitted in the same second from the same process get distinct
/// idempotency keys.
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
    /// When the event was emitted, as Unix epoch seconds. Preserved through [`outbox::relay`], so
    /// this is always when the producer recorded the fact, never when it reached the log.
    #[serde(default)]
    pub emitted_at: u64,
}

/// A typed event payload that owns its topic and can construct the complete bus record.
pub trait Event: Serialize {
    /// The dotted event topic, e.g. `adi.agents.run.finished`.
    const NAME: &'static str;

    /// Build the record with this event's topic, JSON payload, and the current timestamp.
    ///
    /// # Errors
    /// [`Error::InvalidName`] if the topic is unsafe, or [`Error::Encode`] if the payload
    /// cannot be serialized.
    fn to_record(&self) -> Result<EventRecord> {
        validate_name(Self::NAME)?;
        Ok(EventRecord {
            name: Self::NAME.to_string(),
            payload: serde_json::to_string(self).map_err(Error::Encode)?,
            emitted_at: now_unix(),
        })
    }
}

/// One row read back off the log: its [`seq`](Self::seq) (pass to [`Events::ack`] once handled)
/// and the event itself.
#[derive(Debug, Clone)]
pub struct LoggedEvent {
    /// The log's own monotonic position for this row.
    pub seq: u64,
    /// The event.
    pub record: EventRecord,
}

/// The event log: appends and reads rows of the shared `events`/`log.db` database. Cheap to
/// clone; all state is on disk.
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

    /// The `events` module directory: `~/.adi/mono/events` (where `log.db` lives).
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(MODULE).dir().to_path_buf()
    }

    /// Open (and prepare) a connection to the log database.
    fn connect(&self) -> Result<Connection> {
        let module = self.config.module(MODULE);
        module.ensure_dir()?;
        let conn = Connection::open(module.raw_path(DB_FILE))?;
        conn.execute_batch(PRAGMAS)?;
        conn.execute_batch(SCHEMA)?;
        Ok(conn)
    }

    /// Publish `payload` as JSON, discarding both the encoding failure and the append failure.
    ///
    /// The stores emit as a side effect of a mutation that has already been written to disk. The
    /// record is the fact; the event is a notification about it, and a notification that could
    /// fail the operation it describes would mean a project half-created because the log could not
    /// be written. So every store swallows both errors — and each of them was implementing that
    /// same swallow privately, which is a decision worth making once and being able to find.
    pub fn emit_json(&self, name: &str, payload: &impl serde::Serialize) {
        if let Ok(json) = serde_json::to_string(payload) {
            let _ = self.emit(name, json);
        }
    }

    /// Publish a typed event, deriving its topic and JSON payload from the event itself.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe topic, [`Error::Encode`] if the payload or record
    /// cannot be serialized, or [`Error::Sqlite`] on a write failure.
    pub fn emit_event(&self, event: &impl Event) -> Result<()> {
        let record = event.to_record()?;
        self.append_record(&direct_key(), &record)?;
        Ok(())
    }

    /// Publish an event: append one row to the log, under a freshly minted idempotency key (this
    /// is a one-shot publish with nothing to retry, unlike [`outbox::relay`]).
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an empty or unsafe name, or [`Error::Sqlite`] on a write failure.
    pub fn emit(&self, name: &str, payload: impl Into<String>) -> Result<()> {
        validate_name(name)?;
        let record = EventRecord {
            name: name.to_string(),
            payload: payload.into(),
            emitted_at: now_unix(),
        };
        self.append_record(&direct_key(), &record)?;
        Ok(())
    }

    /// Append `record` under `idempotency_key`, or — if that key is already in the log, which
    /// means somebody already appended this exact fact — hand back the `seq` it got the first
    /// time. This is what makes [`outbox::relay`] safe to run twice over the same pending row:
    /// relaying it again is a no-op, not a duplicate event.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe topic, or [`Error::Sqlite`] on a write or read failure
    /// other than the expected unique-constraint hit.
    pub(crate) fn append_record(&self, idempotency_key: &str, record: &EventRecord) -> Result<u64> {
        validate_name(&record.name)?;
        let conn = self.connect()?;
        match conn.execute(
            "INSERT INTO events (idempotency_key, name, payload, emitted_at) VALUES (?1,?2,?3,?4)",
            params![idempotency_key, record.name, record.payload, record.emitted_at],
        ) {
            Ok(_) => Ok(seq_of(conn.last_insert_rowid())),
            Err(e) if is_unique_violation(&e) => Ok(conn.query_row(
                "SELECT seq FROM events WHERE idempotency_key = ?1",
                [idempotency_key],
                |row| row.get::<_, i64>(0),
            )
            .map(seq_of)?),
            Err(e) => Err(e.into()),
        }
    }

    /// Everything a `subscriber` hasn't [`ack`](Self::ack)'d yet, oldest first, capped at `limit`.
    /// A subscriber that has never been seen starts at the very beginning of the log — which is
    /// exactly "catch-up after downtime", whether the downtime was this subscriber never having
    /// run before or the whole app having been off since the event was appended.
    ///
    /// # Errors
    /// [`Error::Sqlite`] on a read failure.
    pub fn catch_up(&self, subscriber: &str, limit: usize) -> Result<Vec<LoggedEvent>> {
        let conn = self.connect()?;
        let after = cursor_on(&conn, subscriber)?;
        let mut stmt = conn.prepare(
            "SELECT seq, name, payload, emitted_at FROM events \
             WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![seq_param(after), seq_param(limit as u64)], |row| {
                Ok(LoggedEvent {
                    seq: seq_of(row.get(0)?),
                    record: EventRecord {
                        name: row.get(1)?,
                        payload: row.get(2)?,
                        emitted_at: seq_of(row.get(3)?),
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Advance `subscriber`'s cursor to (at least) `seq` — advancing to a position already passed
    /// is a no-op, so acking the same batch twice (after a crash between acting on it and acking
    /// it) never moves the cursor backwards.
    ///
    /// # Errors
    /// [`Error::Sqlite`] on a write failure.
    pub fn ack(&self, subscriber: &str, seq: u64) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO event_cursors (subscriber, seq, updated_at) VALUES (?1,?2,?3) \
             ON CONFLICT(subscriber) DO UPDATE SET \
                seq = MAX(seq, excluded.seq), updated_at = excluded.updated_at",
            params![subscriber, seq_param(seq), seq_param(now_unix())],
        )?;
        Ok(())
    }

    /// `subscriber`'s current position — `0` for one that has never caught up or acked.
    ///
    /// # Errors
    /// [`Error::Sqlite`] on a read failure.
    pub fn cursor(&self, subscriber: &str) -> Result<u64> {
        let conn = self.connect()?;
        cursor_on(&conn, subscriber)
    }

    /// Everything past `seq`, oldest first, capped at `limit` — the same read [`catch_up`]
    /// does, but against an explicit position instead of a named subscriber's cursor, and with
    /// no side effect on any cursor. For a consumer that polls its own saved position rather than
    /// sharing one of the bus's own subscriber slots — `adi-workflows`' `ctx.waitFor`, concretely,
    /// which needs to re-check "anything new since I started waiting" on every attempt of a run
    /// without ever acking, since two different runs waiting on the same pattern must each see
    /// every match, not divide them up the way `catch_up`'s at-least-once delivery is built to.
    ///
    /// # Errors
    /// [`Error::Sqlite`] on a read failure.
    pub fn after(&self, seq: u64, limit: usize) -> Result<Vec<LoggedEvent>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT seq, name, payload, emitted_at FROM events \
             WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![seq_param(seq), seq_param(limit as u64)], |row| {
                Ok(LoggedEvent {
                    seq: seq_of(row.get(0)?),
                    record: EventRecord {
                        name: row.get(1)?,
                        payload: row.get(2)?,
                        emitted_at: seq_of(row.get(3)?),
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The log's current tip — the highest `seq` appended so far, or `0` if it's empty. What
    /// `ctx.waitFor` saves the first time it reaches a step, so only events appended *after* that
    /// point ever resolve it (spec section E: "saves the log position when it starts").
    ///
    /// # Errors
    /// [`Error::Sqlite`] on a read failure.
    pub fn head(&self) -> Result<u64> {
        let conn = self.connect()?;
        let seq: Option<i64> = conn.query_row("SELECT MAX(seq) FROM events", [], |row| row.get(0))?;
        Ok(seq.map_or(0, seq_of))
    }

    /// Drop every event older than `max_age` that **every** registered subscriber has already
    /// passed — a subscriber with no cursor row at all (never once caught up) blocks retention
    /// entirely, by design: "never before every cursor has passed" is only honored by refusing to
    /// guess that an unseen subscriber doesn't need the event. Returns how many rows were dropped.
    ///
    /// Call this on a schedule (daily is plenty); it does nothing on its own.
    ///
    /// # Errors
    /// [`Error::Sqlite`] on a read or write failure.
    pub fn retain(&self, max_age: Duration) -> Result<usize> {
        let conn = self.connect()?;
        let min_cursor: Option<i64> =
            conn.query_row("SELECT MIN(seq) FROM event_cursors", [], |row| row.get(0))?;
        let Some(min_cursor) = min_cursor else {
            return Ok(0);
        };
        let cutoff = now_unix().saturating_sub(max_age.as_secs());
        let deleted = conn.execute(
            "DELETE FROM events WHERE seq <= ?1 AND emitted_at <= ?2",
            params![min_cursor, seq_param(cutoff)],
        )?;
        Ok(deleted)
    }
}

/// A fresh idempotency key for a direct (non-outbox) publish: there is nothing to retry, so this
/// only has to be unique, not reproducible.
fn direct_key() -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("direct:{}-{:08}-{seq:08}", now_unix(), std::process::id())
}

/// `subscriber`'s current cursor position on an already-open connection — the shared body behind
/// [`Events::catch_up`] and [`Events::cursor`].
fn cursor_on(conn: &Connection, subscriber: &str) -> Result<u64> {
    let seq: Option<i64> = conn
        .query_row(
            "SELECT seq FROM event_cursors WHERE subscriber = ?1",
            [subscriber],
            |row| row.get(0),
        )
        .optional()?;
    Ok(seq.map_or(0, seq_of))
}

/// Whether a rusqlite error is the unique-constraint hit [`Events::append_record`] treats as "this
/// idempotency key was already appended", rather than a real failure.
fn is_unique_violation(e: &rusqlite::Error) -> bool {
    e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)
}

/// A `seq`/`emitted_at` value is stored as SQLite's signed `INTEGER` but is logically a `u64`
/// counter that never goes negative in practice; this is the one narrowing point for every such
/// column read back off a row.
fn seq_of(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// The inverse of [`seq_of`] — binding a `u64` counter as SQLite's signed `INTEGER` parameter.
fn seq_param(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Reject an event name that isn't a safe dotted topic: non-empty, every character a letter,
/// digit, `.`, `-`, or `_`. The rule matters because the name is matched against subscription
/// patterns and shows up in logs. Public so a producer (or the catalog's coherence test) can check
/// a name against the same rule the bus enforces on [`Events::emit`].
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

    #[derive(Serialize, Deserialize)]
    struct Tick {
        n: usize,
    }

    impl Event for Tick {
        const NAME: &'static str = "test.tick";
    }

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
    fn typed_event_builds_and_logs_a_complete_record() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct ExampleEvent {
            message: String,
        }

        impl Event for ExampleEvent {
            const NAME: &'static str = "adi.example.finished";
        }

        let bus = scratch("typed-roundtrip");
        let event = ExampleEvent {
            message: "a \"quoted\" value, a backslash \\, and a newline\n".into(),
        };
        let before = now_unix();
        let record = event.to_record().expect("build typed record");
        bus.emit_event(&event).expect("emit typed event");
        let after = now_unix();
        let caught_up = bus.catch_up("sub", 10).expect("catch up");
        assert_eq!(caught_up.len(), 1);

        for record in [&record, &caught_up[0].record] {
            assert_eq!(record.name, ExampleEvent::NAME);
            assert!(record.emitted_at > 0);
            assert!((before..=after).contains(&record.emitted_at));
            assert_eq!(
                serde_json::from_str::<ExampleEvent>(&record.payload).expect("decode payload"),
                event,
            );
        }
    }

    #[test]
    fn typed_event_encoding_failure_never_logs() {
        struct UnencodableEvent;

        impl Serialize for UnencodableEvent {
            fn serialize<S: serde::Serializer>(
                &self,
                _serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("intentional encoding failure"))
            }
        }

        impl Event for UnencodableEvent {
            const NAME: &'static str = "adi.example.unencodable";
        }

        let bus = scratch("typed-encode-error");
        assert!(matches!(
            UnencodableEvent.to_record(),
            Err(Error::Encode(_))
        ));
        assert!(matches!(
            bus.emit_event(&UnencodableEvent),
            Err(Error::Encode(_))
        ));
        assert!(bus.catch_up("sub", 10).expect("still empty").is_empty());
    }

    #[test]
    fn typed_event_invalid_name_never_logs() {
        #[derive(Serialize)]
        struct InvalidEvent;

        impl Event for InvalidEvent {
            const NAME: &'static str = "bad name";
        }

        let bus = scratch("typed-invalid-name");
        assert!(matches!(
            InvalidEvent.to_record(),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            bus.emit_event(&InvalidEvent),
            Err(Error::InvalidName(_))
        ));
        assert!(bus.catch_up("sub", 10).expect("still empty").is_empty());
    }

    #[test]
    fn emit_then_catch_up_round_trips_and_ack_advances_the_cursor() {
        let bus = scratch("roundtrip");
        assert!(bus.catch_up("sub", 10).expect("empty catch up").is_empty());

        bus.emit("adi.tasks.created", r#"{"id":"t1"}"#)
            .expect("emit");
        let caught_up = bus.catch_up("sub", 10).expect("catch up");
        assert_eq!(caught_up.len(), 1);
        assert_eq!(caught_up[0].record.name, "adi.tasks.created");
        assert_eq!(caught_up[0].record.payload, r#"{"id":"t1"}"#);
        assert!(caught_up[0].record.emitted_at > 0);

        bus.ack("sub", caught_up[0].seq).expect("ack");
        assert!(bus.catch_up("sub", 10).expect("caught up").is_empty());
        // Acking a position already passed is a no-op, not an error.
        bus.ack("sub", caught_up[0].seq).expect("idempotent ack");
        assert_eq!(bus.cursor("sub").expect("cursor"), caught_up[0].seq);
    }

    #[test]
    fn catch_up_returns_events_in_emission_order() {
        let bus = scratch("order");
        for i in 0..5 {
            bus.emit_event(&Tick { n: i }).expect("emit");
        }
        let sequence: Vec<usize> = bus
            .catch_up("sub", 100)
            .expect("catch up")
            .into_iter()
            .map(|s| {
                serde_json::from_str::<Tick>(&s.record.payload)
                    .expect("tick")
                    .n
            })
            .collect();
        assert_eq!(sequence, vec![0, 1, 2, 3, 4]);
    }

    /// The core reliability contract: two independent subscribers each replay from their own
    /// position, and an event already acked by one is still fresh to the other.
    #[test]
    fn independent_subscribers_each_catch_up_from_their_own_cursor() {
        let bus = scratch("independent");
        bus.emit_event(&Tick { n: 1 }).expect("emit 1");
        bus.emit_event(&Tick { n: 2 }).expect("emit 2");

        let first = bus.catch_up("alpha", 100).expect("alpha catch up");
        assert_eq!(first.len(), 2);
        bus.ack("alpha", first[0].seq).expect("ack just the first");

        // Alpha only acked the first row, so it still owes itself the second.
        let alpha_again = bus.catch_up("alpha", 100).expect("alpha again");
        assert_eq!(alpha_again.len(), 1);
        assert_eq!(alpha_again[0].seq, first[1].seq);

        // Beta has never been seen before, so it starts from the very top — unaffected by what
        // alpha has or hasn't acked.
        let beta = bus.catch_up("beta", 100).expect("beta catch up");
        assert_eq!(beta.len(), 2);
    }

    /// At-least-once, made concrete: a subscriber that reads a batch but crashes before acking it
    /// sees the exact same batch again next time — "double delivery" is expected, not a bug, which
    /// is why consumers must treat re-delivery as safe (idempotency keys are the log's half of
    /// that; a consumer's own effects are its half).
    #[test]
    fn an_unacked_batch_is_delivered_again() {
        let bus = scratch("double-delivery");
        bus.emit("adi.tasks.created", "{}").expect("emit");

        let first_read = bus.catch_up("sub", 10).expect("first read");
        assert_eq!(first_read.len(), 1);
        // No ack: simulates a crash between reading the batch and acting on it.
        let second_read = bus.catch_up("sub", 10).expect("second read");
        assert_eq!(second_read.len(), 1);
        assert_eq!(first_read[0].seq, second_read[0].seq);
        assert_eq!(first_read[0].record, second_read[0].record);
    }

    /// Catch-up after downtime: nothing about replay depends on when the subscriber first looked —
    /// events appended while nobody was watching are exactly as catchable as ones appended a
    /// moment ago.
    #[test]
    fn a_subscriber_that_never_ran_catches_up_from_the_very_start() {
        let bus = scratch("downtime");
        for i in 0..3 {
            bus.emit_event(&Tick { n: i }).expect("emit while nobody was watching");
        }
        // A brand new subscriber showing up well after the fact still gets everything, in order.
        let caught_up = bus.catch_up("late-subscriber", 100).expect("catch up");
        let sequence: Vec<usize> = caught_up
            .iter()
            .map(|s| {
                serde_json::from_str::<Tick>(&s.record.payload)
                    .expect("tick")
                    .n
            })
            .collect();
        assert_eq!(sequence, vec![0, 1, 2]);
    }

    /// `after` is `catch_up` without any subscriber cursor: two independent callers polling the
    /// same explicit position each see every matching row, and neither advances anything.
    #[test]
    fn after_reads_past_an_explicit_position_with_no_cursor_side_effect() {
        let bus = scratch("after");
        bus.emit_event(&Tick { n: 1 }).expect("emit 1");
        let first_seq = bus.after(0, 10).expect("after 0")[0].seq;
        bus.emit_event(&Tick { n: 2 }).expect("emit 2");

        let since_zero = bus.after(0, 10).expect("after 0 again");
        assert_eq!(since_zero.len(), 2);
        let since_first = bus.after(first_seq, 10).expect("after first");
        assert_eq!(since_first.len(), 1);

        // Nothing here touched a cursor at all.
        assert_eq!(bus.cursor("nobody").expect("cursor"), 0);
        assert_eq!(bus.catch_up("nobody", 10).expect("catch up").len(), 2);
    }

    #[test]
    fn head_is_zero_when_empty_and_the_tip_otherwise() {
        let bus = scratch("head");
        assert_eq!(bus.head().expect("head"), 0);
        bus.emit("a", "{}").expect("emit");
        bus.emit("b", "{}").expect("emit");
        let caught = bus.catch_up("sub", 10).expect("catch up");
        assert_eq!(bus.head().expect("head"), caught[1].seq);
    }

    #[test]
    fn an_invalid_name_never_logs() {
        let bus = scratch("invalid");
        assert!(matches!(bus.emit("", "x"), Err(Error::InvalidName(_))));
        assert!(matches!(
            bus.emit("bad name", "x"),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(bus.emit("a/b", "x"), Err(Error::InvalidName(_))));
        assert!(bus.catch_up("sub", 10).expect("still empty").is_empty());
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
    fn retain_never_drops_past_the_slowest_cursor() {
        let bus = scratch("retain-slowest");
        bus.emit_event(&Tick { n: 1 }).expect("emit 1");
        bus.emit_event(&Tick { n: 2 }).expect("emit 2");

        // No cursor has ever been registered: nothing is provably safe to drop.
        assert_eq!(bus.retain(Duration::from_secs(0)).expect("retain"), 0);

        let rows = bus.catch_up("fast", 100).expect("fast catch up");
        bus.ack("fast", rows[1].seq).expect("fast acks everything");
        // `slow` has shown up (so it has a cursor row) but hasn't acked anything yet.
        bus.ack("slow", 0).expect("slow registers at the start");

        assert_eq!(
            bus.retain(Duration::from_secs(0)).expect("retain"),
            0,
            "slow still hasn't passed either row"
        );

        bus.ack("slow", rows[0].seq).expect("slow passes the first");
        assert_eq!(
            bus.retain(Duration::from_secs(0)).expect("retain"),
            1,
            "only the row every cursor has passed is dropped"
        );
    }

    #[test]
    fn retain_respects_the_age_floor_even_once_every_cursor_has_passed() {
        let bus = scratch("retain-age");
        bus.emit_event(&Tick { n: 1 }).expect("emit");
        let rows = bus.catch_up("sub", 10).expect("catch up");
        bus.ack("sub", rows[0].seq).expect("ack");

        // Every cursor has passed it, but it is not old enough yet.
        assert_eq!(
            bus.retain(Duration::from_secs(3600)).expect("retain"),
            0,
            "a one-hour floor must not drop an event emitted moments ago"
        );
        assert_eq!(bus.retain(Duration::from_secs(0)).expect("retain"), 1);
    }
}
