//! The transactional-outbox half of the reliable log: a table a producer adds to **its own**
//! database, so an event can be written in the very same transaction as the state change it
//! describes, then forwarded into [`Events`](crate::Events)'s log once that transaction has
//! committed.
//!
//! A producer:
//! 1. Concats [`SCHEMA`] into its own `CREATE TABLE IF NOT EXISTS` migration, once.
//! 2. Inside the transaction that changes its state, calls [`write`] on that same `tx`.
//! 3. After the transaction commits, calls [`relay`] (best-effort, swallowing failure exactly like
//!    a direct [`Events::emit`](crate::Events::emit) always has) to forward what it just wrote.
//!
//! Step 3 is not the only time a pending row ever gets relayed: any later call to [`relay`] over
//! the same table — from this process the next time it writes, or from another process, or from a
//! periodic sweep the app runs — picks up whatever a crash left behind between steps 2 and 3. That
//! is what makes the gap between "committed locally" and "visible in the log" survive downtime:
//! the fact was never only in memory, it is a row on disk the moment the transaction in step 2
//! commits.

use rusqlite::Connection;

use crate::{Error, EventRecord, Events, Result, validate_name};

/// The local table's name and shape. A producer owns the table (it lives in the producer's own
/// database), but not its shape — every producer's outbox looks exactly like this, which is what
/// lets [`relay`] be generic over any of them.
pub const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS event_outbox (
    id         INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    payload    TEXT    NOT NULL,
    created_at INTEGER NOT NULL
);
";

/// Write one pending event into `tx`'s local outbox table. Call this *inside* the transaction that
/// makes the state change the event describes — that is the entire point: a crash after `tx`
/// commits can no longer lose this event, only delay it reaching the shared log.
///
/// # Errors
/// [`Error::InvalidName`] for an unsafe topic, or [`Error::Sqlite`] on a write failure (most likely
/// because the caller hasn't applied [`SCHEMA`] yet).
pub fn write(tx: &Connection, name: &str, payload: &str) -> Result<()> {
    validate_name(name)?;
    tx.execute(
        "INSERT INTO event_outbox (name, payload, created_at) VALUES (?1,?2,?3)",
        rusqlite::params![name, payload, adi_config::now_unix()],
    )?;
    Ok(())
}

/// Forward up to `limit` pending rows from `local`'s outbox table into `log`, oldest first, then
/// delete each one it successfully forwarded. `source` names the producer (`"tasks"`, `"agents"`,
/// …) and becomes part of the idempotency key (`"{source}#{local id}"`), so relaying the same row
/// twice — the local delete failed, or two processes raced the same backlog — appends it to the
/// log only once; the second attempt finds its own key already there, gets back the same `seq`,
/// and still deletes the now-redundant local row.
///
/// Call this right after the transaction that [`write`] ran inside commits (best-effort — a
/// failure here must never undo the state change, only leave the row for the next relay to find),
/// and again on a schedule so a crash between that commit and this call still gets delivered once
/// something — this process's next write, another process, a periodic sweep — relays the table.
///
/// # Errors
/// [`Error::Sqlite`] on a read or write failure against `local`.
pub fn relay(local: &Connection, log: &Events, source: &str, limit: usize) -> Result<usize> {
    let mut stmt = local.prepare(
        "SELECT id, name, payload, created_at FROM event_outbox ORDER BY id LIMIT ?1",
    )?;
    let pending: Vec<(i64, EventRecord)> = stmt
        .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok((
                row.get(0)?,
                EventRecord {
                    name: row.get(1)?,
                    payload: row.get(2)?,
                    emitted_at: row.get(3)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()
        .map_err(Error::Sqlite)?;
    drop(stmt);

    let mut relayed = 0;
    for (id, record) in pending {
        log.append_record(&format!("{source}#{id}"), &record)?;
        local.execute("DELETE FROM event_outbox WHERE id = ?1", [id])?;
        relayed += 1;
    }
    Ok(relayed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;

    fn scratch_events(tag: &str) -> Events {
        let root = std::env::temp_dir().join(format!(
            "adi-events-outbox-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Events::with_config(Config::with_root(root))
    }

    fn producer_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(SCHEMA).expect("schema");
        conn
    }

    #[test]
    fn a_row_written_in_a_transaction_is_relayed_after_it_commits() {
        let producer = producer_conn();
        let tx = producer.unchecked_transaction().expect("begin");
        write(&tx, "adi.tasks.created", r#"{"id":"t1"}"#).expect("write in the same tx");
        tx.commit().expect("commit");

        let log = scratch_events("basic");
        let relayed = relay(&producer, &log, "tasks", 10).expect("relay");
        assert_eq!(relayed, 1);

        let caught_up = log.catch_up("sub", 10).expect("catch up");
        assert_eq!(caught_up.len(), 1);
        assert_eq!(caught_up[0].record.name, "adi.tasks.created");
        assert_eq!(caught_up[0].record.payload, r#"{"id":"t1"}"#);

        // The local row is gone once relayed, so a repeat relay forwards nothing new.
        assert_eq!(relay(&producer, &log, "tasks", 10).expect("relay again"), 0);
    }

    /// A relay that never ran right after commit (the process died first) is not a lost event: the
    /// row is still sitting in the local table, waiting for whichever relay runs next — simulating
    /// "the app was down" by simply not relaying until well after the write.
    #[test]
    fn a_row_left_unrelayed_by_a_crash_is_caught_up_by_a_later_relay() {
        let producer = producer_conn();
        let tx = producer.unchecked_transaction().expect("begin");
        write(&tx, "adi.tasks.created", "{}").expect("write");
        tx.commit().expect("commit — simulates the process dying before it could relay");

        let log = scratch_events("downtime");
        assert!(log.catch_up("sub", 10).expect("nothing yet").is_empty());

        // Time passes; something else finally relays the backlog.
        assert_eq!(relay(&producer, &log, "tasks", 10).expect("late relay"), 1);
        assert_eq!(log.catch_up("sub", 10).expect("now present").len(), 1);
    }

    /// Relaying the same pending row twice — the local delete lost the race, or two processes both
    /// picked it up — must not double the event in the log.
    #[test]
    fn relaying_the_same_row_twice_appends_it_only_once() {
        let producer = producer_conn();
        let tx = producer.unchecked_transaction().expect("begin");
        write(&tx, "adi.tasks.created", "{}").expect("write");
        tx.commit().expect("commit");

        let log = scratch_events("idempotent");
        // Relay once, but imagine the local DELETE never lands by reinserting the same row with
        // the same id the real table would have assigned it — exercising `relay`'s own dedupe
        // rather than relying on the DELETE actually having failed.
        relay(&producer, &log, "tasks", 10).expect("first relay");
        producer
            .execute(
                "INSERT INTO event_outbox (id, name, payload, created_at) VALUES (1, 'adi.tasks.created', '{}', 0)",
                [],
            )
            .expect("reinsert the same id, as if the delete had failed");
        relay(&producer, &log, "tasks", 10).expect("second relay");

        let caught_up = log.catch_up("sub", 10).expect("catch up");
        assert_eq!(caught_up.len(), 1, "the same fact must not become two events");
    }

    #[test]
    fn relay_preserves_the_original_emitted_at_not_the_relay_time() {
        let producer = producer_conn();
        producer
            .execute(
                "INSERT INTO event_outbox (name, payload, created_at) VALUES ('adi.tasks.created', '{}', 12345)",
                [],
            )
            .expect("seed a row with an old timestamp");

        let log = scratch_events("preserve-time");
        relay(&producer, &log, "tasks", 10).expect("relay");
        let caught_up = log.catch_up("sub", 10).expect("catch up");
        assert_eq!(caught_up[0].record.emitted_at, 12345);
    }

    #[test]
    fn relay_respects_the_limit_and_leaves_the_rest_for_next_time() {
        let producer = producer_conn();
        for i in 0..5 {
            producer
                .execute(
                    "INSERT INTO event_outbox (name, payload, created_at) VALUES (?1, '{}', 0)",
                    [format!("test.item.{i}")],
                )
                .expect("seed");
        }

        let log = scratch_events("limit");
        assert_eq!(relay(&producer, &log, "tasks", 2).expect("first batch"), 2);
        assert_eq!(relay(&producer, &log, "tasks", 2).expect("second batch"), 2);
        assert_eq!(relay(&producer, &log, "tasks", 2).expect("final batch"), 1);
        assert_eq!(log.catch_up("sub", 100).expect("catch up").len(), 5);
    }

    #[test]
    fn an_unsafe_name_refuses_to_write() {
        let producer = producer_conn();
        let tx = producer.unchecked_transaction().expect("begin");
        assert!(matches!(
            write(&tx, "bad name", "{}"),
            Err(Error::InvalidName(_))
        ));
    }
}
