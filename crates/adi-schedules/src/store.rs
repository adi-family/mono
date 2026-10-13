//! `schedules.db`, under the `schedules` module dir — one file of its own, for the same reason
//! `adi-tasks`' `tasks.db` and `adi-agents`' `sessions.db` are each their own file rather than
//! sharing the platform-wide `db/global.db`: this is bookkeeping the dispatcher-equivalent sweep
//! owns, not a user's own data.
//!
//! Two tables, and the whole point of this module is that both are updated in the same write
//! transaction as the slot computation that decided them:
//!
//! * `schedule_cursor` — one row per workflow, the last slot [`crate::slots::due`] resolved to,
//!   fire or skip. The anchor every later evaluation computes *from*.
//! * `poured_runs` — one row per `wf@slot` run key that has ever actually fired, the unique claim
//!   the spec asks for. A `UNIQUE` index is the real guarantee (two processes racing the same
//!   slot can't both win); the single-process sweep loop calling [`crate::tick`] never needs to
//!   race itself, but nothing here assumes that.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use adi_config::Config;

use crate::error::{Error, Result};
use crate::rule::ScheduleRule;
use crate::slots;

/// The config module this store persists under: `~/.adi/mono/schedules`.
const MODULE: &str = "schedules";
const DB_FILE: &str = "schedules.db";

const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma synchronous = NORMAL;\n";

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS schedule_cursor (
    workflow      TEXT    NOT NULL PRIMARY KEY,
    last_slot_utc TEXT    NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS poured_runs (
    run_key    TEXT    NOT NULL PRIMARY KEY,
    workflow   TEXT    NOT NULL,
    slot_utc   TEXT    NOT NULL,
    poured_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS poured_runs_workflow ON poured_runs (workflow);
";

/// One claimed, due-to-fire slot — `run_key` already belongs to this call and no other, by the
/// time it comes back from [`ScheduleStore::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub workflow: String,
    pub slot: DateTime<Utc>,
    /// `${workflow}@${slot}` — spec section D's run key, RFC 3339 UTC to the second.
    pub run_key: String,
}

fn run_key(workflow: &str, slot: DateTime<Utc>) -> String {
    format!(
        "{workflow}@{}",
        slot.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    )
}

/// The schedule sweep's own store: the cursor every rule resolves against, and the claim that
/// makes a `wf@slot` run key fire at most once, ever.
#[derive(Debug, Clone)]
pub struct ScheduleStore {
    config: Config,
}

impl Default for ScheduleStore {
    fn default() -> Self {
        Self::open()
    }
}

impl ScheduleStore {
    /// Open the store backed by the standard config (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the store backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The `schedules` module directory: `~/.adi/mono/schedules` (where `schedules.db` lives).
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(MODULE).dir().to_path_buf()
    }

    fn db_path(&self) -> PathBuf {
        self.dir().join(DB_FILE)
    }

    fn connect(&self) -> Result<Connection> {
        let path = self.db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Store(anyhow::anyhow!("couldn't create {}: {e}", parent.display()))
            })?;
        }
        let conn = Connection::open(&path)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't open {}: {e}", path.display())))?;
        conn.execute_batch(PRAGMAS).map_err(|e| {
            Error::Store(anyhow::anyhow!(
                "couldn't configure the schedule database: {e}"
            ))
        })?;
        conn.execute_batch(SCHEMA).map_err(|e| {
            Error::Store(anyhow::anyhow!("couldn't create the schedule schema: {e}"))
        })?;
        Ok(conn)
    }

    /// `workflow`'s cursor — the last slot any evaluation resolved to, fire or skip — or `None`
    /// for a workflow this store has never evaluated at all.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn cursor(&self, workflow: &str) -> Result<Option<DateTime<Utc>>> {
        let conn = self.connect()?;
        cursor_on(&conn, workflow)
    }

    /// Every `wf@slot` run key this store has ever claimed for `workflow`, oldest first — the
    /// audit trail behind [`resolve`](Self::resolve)'s claim, and what a test asserts against to
    /// show a slot fired exactly once.
    ///
    /// # Errors
    /// [`Error::Store`] on a read failure.
    pub fn poured(&self, workflow: &str) -> Result<Vec<Due>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT run_key, workflow, slot_utc FROM poured_runs \
                 WHERE workflow = ?1 ORDER BY poured_at, run_key",
            )
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read poured runs: {e}")))?;
        let rows = stmt
            .query_map([workflow], |row| {
                let run_key: String = row.get(0)?;
                let workflow: String = row.get(1)?;
                let slot_utc: String = row.get(2)?;
                Ok((run_key, workflow, slot_utc))
            })
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read poured runs: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            let (run_key, workflow, slot_utc) =
                row.map_err(|e| Error::Store(anyhow::anyhow!("couldn't read a poured run: {e}")))?;
            out.push(Due {
                workflow,
                slot: parse_slot(&slot_utc)?,
                run_key,
            });
        }
        Ok(out)
    }

    /// Resolve `rule` against this store's own cursor, advance the cursor, and claim the run key —
    /// all inside one write transaction, so a crash between "decided to fire" and "recorded that
    /// we did" can't leave the two disagreeing. Returns the claimed [`Due`] only when this call is
    /// the one allowed to pour it: nothing due, a backlog dropped under `catchUp: skip`, or a slot
    /// some earlier call already claimed (another process racing the same workflow, in practice)
    /// all come back `None` alike — there is nothing left for the caller to do differently between
    /// them.
    ///
    /// # Errors
    /// [`Error::InvalidCron`] / [`Error::UnknownTimezone`] if `rule` itself doesn't parse,
    /// [`Error::Store`] on a read/write failure.
    pub fn resolve(&self, rule: &ScheduleRule, now: DateTime<Utc>) -> Result<Option<Due>> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't start a transaction: {e}")))?;

        let last = cursor_on(&tx, &rule.workflow)?;
        let resolved = slots::due(rule, last, now)?;

        if let Some(advance_to) = resolved.advance_to {
            tx.execute(
                "INSERT INTO schedule_cursor (workflow, last_slot_utc, updated_at) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT (workflow) DO UPDATE SET \
                     last_slot_utc = excluded.last_slot_utc, updated_at = excluded.updated_at",
                rusqlite::params![
                    rule.workflow,
                    advance_to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    now.timestamp(),
                ],
            )
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't advance the cursor: {e}")))?;
        }

        let due = match resolved.fire {
            None => None,
            Some(slot) => {
                let key = run_key(&rule.workflow, slot);
                let claimed = tx
                    .execute(
                        "INSERT OR IGNORE INTO poured_runs (run_key, workflow, slot_utc, poured_at) \
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![
                            key,
                            rule.workflow,
                            slot.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                            now.timestamp(),
                        ],
                    )
                    .map_err(|e| Error::Store(anyhow::anyhow!("couldn't claim a run key: {e}")))?;
                (claimed == 1).then_some(Due {
                    workflow: rule.workflow.clone(),
                    slot,
                    run_key: key,
                })
            }
        };

        tx.commit().map_err(|e| {
            Error::Store(anyhow::anyhow!(
                "couldn't commit the schedule transaction: {e}"
            ))
        })?;
        Ok(due)
    }
}

fn cursor_on(conn: &Connection, workflow: &str) -> Result<Option<DateTime<Utc>>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT last_slot_utc FROM schedule_cursor WHERE workflow = ?1",
            [workflow],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't read the cursor: {e}")))?;
    stored.map(|s| parse_slot(&s)).transpose()
}

fn parse_slot(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't parse a stored slot {s:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn scratch(tag: &str) -> ScheduleStore {
        let root = std::env::temp_dir().join(format!(
            "adi-schedules-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        ScheduleStore::with_config(Config::with_root(root))
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("valid instant")
    }

    #[test]
    fn a_fresh_workflow_has_no_cursor_and_resolving_it_sets_one_with_no_fire() {
        let store = scratch("fresh");
        let rule = ScheduleRule::new("wf", "0 * * * * *", "UTC");
        assert_eq!(store.cursor("wf").expect("read"), None);

        let now = utc(2026, 1, 1, 12, 0, 30);
        assert_eq!(store.resolve(&rule, now).expect("resolve"), None);
        assert_eq!(store.cursor("wf").expect("read"), Some(now));
        assert!(store.poured("wf").expect("poured").is_empty());
    }

    #[test]
    fn a_due_slot_is_claimed_exactly_once_even_if_resolved_twice() {
        let store = scratch("claim-once");
        let rule = ScheduleRule::new("wf", "0 * * * * *", "UTC");
        store
            .resolve(&rule, utc(2026, 1, 1, 12, 0, 0))
            .expect("seed cursor");

        let now = utc(2026, 1, 1, 12, 1, 30);
        let first = store.resolve(&rule, now).expect("resolve").expect("due");
        assert_eq!(first.slot, utc(2026, 1, 1, 12, 1, 0));
        assert_eq!(first.run_key, "wf@2026-01-01T12:01:00Z");

        // Simulate a racing second claim attempt at the *same* slot by rewinding the cursor back:
        // the unique index on `poured_runs`, not the cursor, is what must refuse the double-fire.
        let conn = store.connect().expect("connect");
        conn.execute(
            "UPDATE schedule_cursor SET last_slot_utc = ?1 WHERE workflow = 'wf'",
            [utc(2026, 1, 1, 12, 0, 0).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)],
        )
        .expect("rewind");
        assert_eq!(
            store.resolve(&rule, now).expect("resolve again"),
            None,
            "the same slot must never be claimed twice"
        );
        assert_eq!(store.poured("wf").expect("poured").len(), 1);
    }

    #[test]
    fn two_workflows_keep_independent_cursors() {
        let store = scratch("independent");
        let a = ScheduleRule::new("a", "0 * * * * *", "UTC");
        let b = ScheduleRule::new("b", "0 * * * * *", "UTC");
        store.resolve(&a, utc(2026, 1, 1, 0, 0, 0)).expect("seed a");
        store.resolve(&b, utc(2026, 1, 1, 6, 0, 0)).expect("seed b");

        assert_eq!(
            store.cursor("a").expect("a"),
            Some(utc(2026, 1, 1, 0, 0, 0))
        );
        assert_eq!(
            store.cursor("b").expect("b"),
            Some(utc(2026, 1, 1, 6, 0, 0))
        );
    }

    #[test]
    fn a_backlog_claims_only_the_slot_it_actually_fires() {
        let store = scratch("backlog-claim");
        let rule = ScheduleRule::new("wf", "0 * * * * *", "UTC");
        store
            .resolve(&rule, utc(2026, 1, 1, 12, 0, 0))
            .expect("seed cursor");

        // Five minutes with no tick: a real backlog.
        let due = store
            .resolve(&rule, utc(2026, 1, 1, 12, 5, 30))
            .expect("resolve")
            .expect("one fire");
        assert_eq!(due.slot, utc(2026, 1, 1, 12, 5, 0));
        assert_eq!(
            store.poured("wf").expect("poured").len(),
            1,
            "never one row per missed slot"
        );
    }
}
