//! The shared record of "this credential is spent until T".
//!
//! # Why it is shared
//!
//! A usage limit is a fact about a *subscription*, not about the run that happened to trip it. With
//! sixteen agents in flight, the limit is discovered sixteen times unless somebody writes it down:
//! fifteen wasted turns, each one a real model call that fails. So the moment one run reads
//! "usage limit reached" off a backend, it records a hold here, and every other run — already
//! running, or launched a minute later — skips that row without spending a turn on it.
//!
//! The key is therefore **the credential, not the backend and not the agent**. Two backends naming
//! the same settings file are one subscription and share a hold; an agent has nothing to do with
//! it. A hold also carries a model, because a five-hour Opus cap does not stop Sonnet on the same
//! login — see [`HoldScope`](crate::llm::HoldScope). A login-scoped hold (a dead token) is stored
//! with an empty model and shadows every model on that credential.
//!
//! # Where it lives, and why not `global.db`
//!
//! Its own database, `llm/holds.db`, opened WAL so the app, the CLI, every run and the prober can
//! all reach it at once. Deliberately *not* the shared `db/global.db`: that store is handed to
//! tools and shown on the Database page, where a `DROP TABLE` from somebody's own SQL is an
//! ordinary thing to type, and platform bookkeeping that silently disappears would turn failover
//! off without a word. The session store makes the same call for the same reason (see
//! [`store::db`](crate::store)).
//!
//! # Expiry, and the one thing that must not happen
//!
//! A hold blocks a row only while `until` is still in the future. Once it passes, the row is *due*:
//! the [prober](crate::llm) sends one cheap request and either releases the hold or extends it with
//! backoff, so in normal operation a conversation never meets a backend whose recovery has not
//! already been established — which is the requirement, because a turn that discovers a backend is
//! back has paid for that discovery with a failed turn.
//!
//! A due hold does not keep blocking, though, and that is on purpose. If the prober is down, the
//! choice is between a chain stuck forever on a limit that lifted hours ago and a single turn
//! paying to find out. The second is plainly the lesser harm, and a backend that is still limited
//! simply re-trips its own rule and is held again with a longer wait.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use adi_config::Config;

use crate::error::{Error, Result};
use crate::llm::LLM_MODULE;
use crate::llm::backend::LimitClass;

/// The database file, under the `llm/` module directory.
const DB_FILE: &str = "holds.db";

/// Applied to every connection, in this order. `busy_timeout` must lead: switching journal mode
/// takes a lock of its own and would otherwise fail outright against a store another process is
/// mid-write on. (The session store and `adi-db` document the same trap.)
const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma synchronous = NORMAL;\n";

/// The table, created on open so any process may be the one that makes it.
///
/// Keyed on `(credential, model)` so recording the same limit twice updates one row rather than
/// growing a log — the question asked of this table is always "is it held *now*", never "how often
/// has it been".
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS llm_holds (
    credential TEXT    NOT NULL,
    model      TEXT    NOT NULL DEFAULT '',
    class      TEXT    NOT NULL,
    until      INTEGER NOT NULL,
    reason     TEXT    NOT NULL DEFAULT '',
    set_by     TEXT    NOT NULL DEFAULT '',
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (credential, model)
);
CREATE INDEX IF NOT EXISTS llm_holds_until ON llm_holds (until);";

/// What a hold is recorded against: a credential, and the model it stops.
///
/// An empty [`model`](Self::model) is the login-scoped form — every model on that credential.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HoldKey {
    /// Who is paying. See [`LlmBackendManifest::credential`](crate::llm::LlmBackendManifest::credential).
    pub credential: String,
    /// The model this concerns, or empty for the whole login.
    pub model: String,
}

impl HoldKey {
    /// A model-scoped key.
    #[must_use]
    pub fn new(credential: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            credential: credential.into(),
            model: model.into(),
        }
    }

    /// The login-scoped key for the same credential — what an `auth` failure is recorded under, and
    /// what every model-scoped lookup also has to consult.
    #[must_use]
    pub fn login(credential: impl Into<String>) -> Self {
        Self {
            credential: credential.into(),
            model: String::new(),
        }
    }
}

/// One recorded hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hold {
    /// What it is recorded against.
    #[serde(flatten)]
    pub key: HoldKey,
    /// Why it is held — which decides whether a run reroutes or asks.
    pub class: LimitClass,
    /// When it may be tried again, in unix seconds.
    pub until: u64,
    /// The provider's own words, kept verbatim. The evidence a human reads when asking why their
    /// agent moved, and the only defence against a regex that matched something it should not have.
    pub reason: String,
    /// Who discovered it — `<agent>/<run id>`, or `prober`.
    pub set_by: String,
    /// How many times the prober has found it still held. Drives the backoff.
    pub attempts: u32,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Hold {
    /// Whether this is still blocking at `now`.
    #[must_use]
    pub fn blocks_at(&self, now: u64) -> bool {
        self.until > now
    }

    /// How it reads in a switch notice: `limited until 14:00`.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("limited until {}", clock(self.until))
    }
}

/// A unix timestamp as a wall clock `HH:MM UTC` — how a notice names a reset time.
///
/// Rendered in UTC, and **labelled** as UTC, because this workspace carries no date library and a
/// hand-rolled local offset would be wrong for half the year in most of the world. An unlabelled
/// "until 14:00" that is silently three hours out is worse than a labelled one the reader can
/// convert: the whole value of the notice is that somebody can check whether the provider's reset
/// time has passed.
#[must_use]
pub fn clock(unix_seconds: u64) -> String {
    let secs_of_day = unix_seconds % 86_400;
    let hours = secs_of_day / 3_600;
    let minutes = (secs_of_day % 3_600) / 60;
    format!("{hours:02}:{minutes:02} UTC")
}

/// The hold store.
#[derive(Debug, Clone)]
pub struct Holds {
    path: PathBuf,
}

impl Holds {
    /// Open the store under a config root.
    #[must_use]
    pub fn with_config(config: &Config) -> Self {
        Self {
            path: config.module(LLM_MODULE).dir().join(DB_FILE),
        }
    }

    /// Open the store at an explicit path — for the prober, and for tests.
    #[must_use]
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Where the database lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn conn(&self) -> Result<Connection> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&self.path).map_err(|e| sql_err("open", &e))?;
        conn.execute_batch(PRAGMAS)
            .map_err(|e| sql_err("configure", &e))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| sql_err("create the schema in", &e))?;
        Ok(conn)
    }

    /// Record a hold, replacing any existing one for the same key.
    ///
    /// Replacing rather than appending is deliberate: a second discovery of the same limit is the
    /// same fact, and what a run needs to know is when it lifts, not how many agents tripped it.
    ///
    /// # Errors
    /// [`Error::Session`] on any store failure.
    pub fn hold(&self, hold: &Hold) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO llm_holds
                 (credential, model, class, until, reason, set_by, attempts, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT (credential, model) DO UPDATE SET
                 class = excluded.class,
                 until = excluded.until,
                 reason = excluded.reason,
                 set_by = excluded.set_by,
                 attempts = excluded.attempts,
                 updated_at = excluded.updated_at",
            params![
                hold.key.credential,
                hold.key.model,
                class_name(hold.class),
                hold.until,
                hold.reason,
                hold.set_by,
                hold.attempts,
                now_unix(),
            ],
        )
        .map_err(|e| sql_err("write to", &e))?;
        Ok(())
    }

    /// The hold blocking `key` right now, if any.
    ///
    /// Consults both the model-scoped hold and the login-scoped one, because a dead token stops
    /// every model on that credential and a caller asking about one model still has to be told.
    ///
    /// # Errors
    /// [`Error::Session`] on any store failure.
    pub fn blocking(&self, key: &HoldKey) -> Result<Option<Hold>> {
        let now = now_unix();
        let conn = self.conn()?;
        let mut statement = conn
            .prepare(
                "SELECT credential, model, class, until, reason, set_by, attempts, created_at, updated_at
                 FROM llm_holds
                 WHERE credential = ?1 AND (model = ?2 OR model = '') AND until > ?3
                 ORDER BY until DESC
                 LIMIT 1",
            )
            .map_err(|e| sql_err("read from", &e))?;
        let mut rows = statement
            .query(params![key.credential, key.model, now])
            .map_err(|e| sql_err("read from", &e))?;
        match rows.next().map_err(|e| sql_err("read from", &e))? {
            Some(row) => Ok(Some(read_hold(row)?)),
            None => Ok(None),
        }
    }

    /// Every hold on record, still blocking or merely due, newest deadline first — what the panel
    /// shows and what the prober walks.
    ///
    /// # Errors
    /// [`Error::Session`] on any store failure.
    pub fn all(&self) -> Result<Vec<Hold>> {
        let conn = self.conn()?;
        let mut statement = conn
            .prepare(
                "SELECT credential, model, class, until, reason, set_by, attempts, created_at, updated_at
                 FROM llm_holds ORDER BY until DESC",
            )
            .map_err(|e| sql_err("read from", &e))?;
        let mut rows = statement.query([]).map_err(|e| sql_err("read from", &e))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| sql_err("read from", &e))? {
            out.push(read_hold(row)?);
        }
        Ok(out)
    }

    /// The holds whose time is up — the prober's work queue.
    ///
    /// # Errors
    /// [`Error::Session`] on any store failure.
    pub fn due(&self) -> Result<Vec<Hold>> {
        let now = now_unix();
        Ok(self
            .all()?
            .into_iter()
            .filter(|hold| !hold.blocks_at(now))
            .collect())
    }

    /// Release a hold, returning whether there was one.
    ///
    /// # Errors
    /// [`Error::Session`] on any store failure.
    pub fn release(&self, key: &HoldKey) -> Result<bool> {
        let conn = self.conn()?;
        let removed = conn
            .execute(
                "DELETE FROM llm_holds WHERE credential = ?1 AND model = ?2",
                params![key.credential, key.model],
            )
            .map_err(|e| sql_err("write to", &e))?;
        Ok(removed > 0)
    }

    /// Push a hold's deadline out after a probe found it still limited, and count the attempt.
    ///
    /// The wait doubles each time, capped, so a backend that is out for the day is not probed every
    /// four hours forever — and a probe is a real model call, however small.
    ///
    /// # Errors
    /// [`Error::Session`] on any store failure.
    pub fn extend(&self, key: &HoldKey, base_seconds: u64) -> Result<Option<Hold>> {
        let conn = self.conn()?;
        let existing = {
            let mut statement = conn
                .prepare(
                    "SELECT credential, model, class, until, reason, set_by, attempts, created_at, updated_at
                     FROM llm_holds WHERE credential = ?1 AND model = ?2",
                )
                .map_err(|e| sql_err("read from", &e))?;
            let mut rows = statement
                .query(params![key.credential, key.model])
                .map_err(|e| sql_err("read from", &e))?;
            match rows.next().map_err(|e| sql_err("read from", &e))? {
                Some(row) => read_hold(row)?,
                None => return Ok(None),
            }
        };

        let attempts = existing.attempts.saturating_add(1);
        let wait = backoff(base_seconds, attempts);
        let until = now_unix().saturating_add(wait);
        conn.execute(
            "UPDATE llm_holds SET until = ?3, attempts = ?4, updated_at = ?5
             WHERE credential = ?1 AND model = ?2",
            params![key.credential, key.model, until, attempts, now_unix()],
        )
        .map_err(|e| sql_err("write to", &e))?;
        Ok(Some(Hold {
            until,
            attempts,
            ..existing
        }))
    }

    /// Drop every hold. The operator's "I have topped up, stop waiting" button.
    ///
    /// # Errors
    /// [`Error::Session`] on any store failure.
    pub fn clear(&self) -> Result<usize> {
        let conn = self.conn()?;
        let removed = conn
            .execute("DELETE FROM llm_holds", [])
            .map_err(|e| sql_err("write to", &e))?;
        Ok(removed)
    }
}

/// The longest a hold is ever pushed out to. Beyond a day, a backend is not rate limited — it is
/// broken or unpaid, and that is a thing to tell somebody about rather than to keep polling.
pub const MAX_BACKOFF: u64 = 24 * 60 * 60;

/// The wait after `attempts` failed probes: doubling, capped at [`MAX_BACKOFF`].
#[must_use]
pub fn backoff(base_seconds: u64, attempts: u32) -> u64 {
    let factor = 1_u64 << attempts.min(16);
    base_seconds.saturating_mul(factor).min(MAX_BACKOFF)
}

fn read_hold(row: &rusqlite::Row<'_>) -> Result<Hold> {
    let get = |index: usize| -> Result<String> { row.get(index).map_err(|e| sql_err("read from", &e)) };
    let get_u64 = |index: usize| -> Result<u64> { row.get(index).map_err(|e| sql_err("read from", &e)) };
    Ok(Hold {
        key: HoldKey {
            credential: get(0)?,
            model: get(1)?,
        },
        class: class_from(&get(2)?),
        until: get_u64(3)?,
        reason: get(4)?,
        set_by: get(5)?,
        attempts: u32::try_from(get_u64(6)?).unwrap_or(u32::MAX),
        created_at: get_u64(7)?,
        updated_at: get_u64(8)?,
    })
}

fn class_name(class: LimitClass) -> &'static str {
    match class {
        LimitClass::Quota => "quota",
        LimitClass::Rate => "rate",
        LimitClass::Auth => "auth",
        LimitClass::Transient => "transient",
        LimitClass::Unknown => "unknown",
    }
}

fn class_from(name: &str) -> LimitClass {
    match name {
        "quota" => LimitClass::Quota,
        "rate" => LimitClass::Rate,
        "auth" => LimitClass::Auth,
        "transient" => LimitClass::Transient,
        _ => LimitClass::Unknown,
    }
}

fn sql_err(doing: &str, e: &rusqlite::Error) -> Error {
    Error::Session(format!("couldn't {doing} the LLM hold store: {e}"))
}

fn now_unix() -> u64 {
    adi_config::now_unix()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Holds {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-llm-holds-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        Holds::with_path(root.join("holds.db"))
    }

    fn hold_for(credential: &str, model: &str, until: u64) -> Hold {
        Hold {
            key: HoldKey::new(credential, model),
            class: LimitClass::Quota,
            until,
            reason: "usage limit reached, resets at 14:00".into(),
            set_by: "adi-agent/r-1".into(),
            attempts: 0,
            created_at: now_unix(),
            updated_at: now_unix(),
        }
    }

    #[test]
    fn a_hold_blocks_its_own_key_until_it_expires() {
        let store = scratch("blocks");
        let key = HoldKey::new("settings:/home/me/.claude/a.json", "claude-opus-5");
        assert!(store.blocking(&key).expect("query").is_none());

        store
            .hold(&hold_for(&key.credential, &key.model, now_unix() + 3_600))
            .expect("hold");
        let found = store.blocking(&key).expect("query").expect("held");
        assert_eq!(found.class, LimitClass::Quota);
        assert!(found.reason.contains("usage limit"));

        // An expired hold no longer blocks — see the module docs on why a due hold gives way.
        store
            .hold(&hold_for(&key.credential, &key.model, now_unix() - 1))
            .expect("hold");
        assert!(store.blocking(&key).expect("query").is_none());
    }

    /// The heart of it: one run's discovery spares every other run, because the key is the
    /// credential rather than the backend or the agent.
    #[test]
    fn one_credential_is_held_for_every_backend_naming_it() {
        let store = scratch("shared");
        let credential = "settings:/home/me/.claude/anthropic.json";
        store
            .hold(&hold_for(credential, "claude-opus-5", now_unix() + 600))
            .expect("hold");

        // A second agent, a different backend id, the same subscription and model.
        assert!(
            store
                .blocking(&HoldKey::new(credential, "claude-opus-5"))
                .expect("query")
                .is_some()
        );
        // A different model on the same subscription is untouched.
        assert!(
            store
                .blocking(&HoldKey::new(credential, "claude-sonnet-5"))
                .expect("query")
                .is_none(),
            "an Opus quota must not stop the Sonnet row beside it"
        );
    }

    /// A dead token is not a model's problem — it stops everything on that login.
    #[test]
    fn a_login_scoped_hold_shadows_every_model() {
        let store = scratch("login-scope");
        let credential = "zai|Z_AI_API_KEY";
        let mut hold = hold_for(credential, "", now_unix() + 600);
        hold.class = LimitClass::Auth;
        store.hold(&hold).expect("hold");

        for model in ["glm-5.3", "glm-4.6", ""] {
            let found = store
                .blocking(&HoldKey::new(credential, model))
                .expect("query")
                .expect("held");
            assert_eq!(found.class, LimitClass::Auth, "model {model}");
        }
    }

    #[test]
    fn recording_the_same_limit_twice_updates_one_row() {
        let store = scratch("upsert");
        let key = HoldKey::new("cred", "m");
        store.hold(&hold_for("cred", "m", now_unix() + 60)).expect("first");
        store.hold(&hold_for("cred", "m", now_unix() + 600)).expect("second");
        assert_eq!(store.all().expect("all").len(), 1);
        assert!(store.blocking(&key).expect("query").expect("held").until > now_unix() + 500);
    }

    #[test]
    fn releasing_reports_whether_there_was_one() {
        let store = scratch("release");
        let key = HoldKey::new("cred", "m");
        store.hold(&hold_for("cred", "m", now_unix() + 60)).expect("hold");
        assert!(store.release(&key).expect("release"));
        assert!(!store.release(&key).expect("release again"));
        assert!(store.blocking(&key).expect("query").is_none());
    }

    #[test]
    fn the_due_queue_holds_only_what_has_expired() {
        let store = scratch("due");
        store.hold(&hold_for("a", "m", now_unix() + 600)).expect("live");
        store.hold(&hold_for("b", "m", now_unix() - 5)).expect("expired");
        let due = store.due().expect("due");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].key.credential, "b");
    }

    #[test]
    fn extending_counts_the_attempt_and_doubles_the_wait() {
        let store = scratch("extend");
        let key = HoldKey::new("cred", "m");
        store.hold(&hold_for("cred", "m", now_unix() - 1)).expect("hold");

        let first = store.extend(&key, 60).expect("extend").expect("present");
        assert_eq!(first.attempts, 1);
        let second = store.extend(&key, 60).expect("extend").expect("present");
        assert_eq!(second.attempts, 2);
        assert!(
            second.until > first.until,
            "each failed probe waits longer than the last"
        );
    }

    #[test]
    fn extending_something_that_is_not_held_says_so() {
        let store = scratch("extend-missing");
        assert!(
            store
                .extend(&HoldKey::new("nobody", "m"), 60)
                .expect("extend")
                .is_none()
        );
    }

    #[test]
    fn backoff_doubles_and_then_stops() {
        assert_eq!(backoff(60, 0), 60);
        assert_eq!(backoff(60, 1), 120);
        assert_eq!(backoff(60, 2), 240);
        assert_eq!(backoff(60, 30), MAX_BACKOFF, "capped, not unbounded");
    }

    #[test]
    fn clearing_drops_everything() {
        let store = scratch("clear");
        store.hold(&hold_for("a", "m", now_unix() + 60)).expect("a");
        store.hold(&hold_for("b", "m", now_unix() + 60)).expect("b");
        assert_eq!(store.clear().expect("clear"), 2);
        assert!(store.all().expect("all").is_empty());
    }

    #[test]
    fn a_hold_reads_as_a_time_in_a_notice() {
        let hold = hold_for("a", "m", 50_400); // 14:00 on day zero
        assert_eq!(hold.describe(), "limited until 14:00 UTC");
    }

    /// Every class survives the round trip through its stored name, including the default.
    #[test]
    fn classes_round_trip_through_the_store() {
        let store = scratch("classes");
        for class in [
            LimitClass::Quota,
            LimitClass::Rate,
            LimitClass::Auth,
            LimitClass::Transient,
            LimitClass::Unknown,
        ] {
            let mut hold = hold_for("cred", "m", now_unix() + 600);
            hold.class = class;
            store.hold(&hold).expect("hold");
            assert_eq!(
                store
                    .blocking(&HoldKey::new("cred", "m"))
                    .expect("query")
                    .expect("held")
                    .class,
                class
            );
        }
    }
}
