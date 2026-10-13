//! What the [dispatch tick](crate::dispatch) remembers about one task, across ticks and across a
//! restart: the `task_dispatch` table, one row per task id.
//!
//! Deliberately not a count of runs — a run is pruned past [`MAX_SESSIONS`](super::MAX_SESSIONS)
//! and a session opened before a restart is still a session, so "how many times has this been
//! tried" has to be a number the dispatcher owns and nothing else can quietly move. Everything
//! else about a task (ready, blocked, done) stays fully computed in `adi-tasks`; this is the one
//! thing the dispatcher itself must remember, because nothing else remembers it for it.

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;

use super::db::sql_err;

/// One task's dispatch bookkeeping. `task_id` is foreign to this database — `adi-tasks` owns the
/// task itself — so there is no foreign key, only a plain primary key on a string nothing else
/// here resolves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DispatchRow {
    pub task_id: String,
    /// The agent most recently dispatched to — the task's own `assignee` at the moment of
    /// [`run_id`](Self::run_id)'s launch.
    pub agent: String,
    /// The most recent attempt's run id. Never cleared once set, even after that run finishes:
    /// it is also the conversation a stuck ask is raised into, and a finished run is still the
    /// right one to ask through until a fresh attempt replaces it.
    pub run_id: String,
    /// Which `run_id` has already had its outcome folded into [`attempts`](Self::attempts) /
    /// [`launch_failures`](Self::launch_failures) — so a run that is finished but not yet
    /// evaluated is counted exactly once, on the tick that first notices it is no longer working.
    pub counted_run_id: String,
    /// Counted attempts toward the retry budget. Platform kills/restarts and `api_error` endings
    /// do not increment this — see `crate::dispatch`.
    pub attempts: u32,
    /// Consecutive-enough `launch_failed` endings — a tighter budget than
    /// [`attempts`](Self::attempts), because a launch that cannot even start rarely fixes itself
    /// on a bare retry.
    pub launch_failures: u32,
    /// Unix seconds of the last dispatch attempt — what the 30s floor and the derived backoff are
    /// measured from.
    pub last_attempt_at: u64,
    /// The pending ask raised for this task while stuck, if one is out. Read back on the next tick
    /// to see whether it has been answered, rather than raising a second one.
    pub stuck_ask_id: String,
    /// The silence alarm already raised for [`run_id`](Self::run_id), if one has been — at most one
    /// per run, however long it stays silent after being answered. Cleared whenever `run_id`
    /// changes (a fresh dispatch is a fresh run, and gets its own alarm if it earns one).
    pub silence_ask_id: String,
}

const COLUMNS: &str = "task_id, agent, run_id, counted_run_id, attempts, launch_failures, \
                       last_attempt_at, stuck_ask_id, silence_ask_id";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatchRow> {
    Ok(DispatchRow {
        task_id: row.get(0)?,
        agent: row.get(1)?,
        run_id: row.get(2)?,
        counted_run_id: row.get(3)?,
        attempts: row.get(4)?,
        launch_failures: row.get(5)?,
        last_attempt_at: row.get(6)?,
        stuck_ask_id: row.get(7)?,
        silence_ask_id: row.get(8)?,
    })
}

/// This task's dispatch row, or `None` for one the dispatcher has never touched.
pub(super) fn get(conn: &Connection, task_id: &str) -> Result<Option<DispatchRow>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM task_dispatch WHERE task_id = ?1"),
        [task_id],
        from_row,
    )
    .optional()
    .map_err(|e| sql_err("read a dispatch row from", e))
}

/// Write `row` back whole — the dispatcher always has the full row in hand (it just read or built
/// it), so there is no partial-update shape to get wrong here the way there is for a goal or a
/// task's own fields.
pub(super) fn save(conn: &Connection, row: &DispatchRow) -> Result<()> {
    conn.execute(
        "INSERT INTO task_dispatch (task_id, agent, run_id, counted_run_id, attempts, \
             launch_failures, last_attempt_at, stuck_ask_id, silence_ask_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (task_id) DO UPDATE SET
             agent = excluded.agent,
             run_id = excluded.run_id,
             counted_run_id = excluded.counted_run_id,
             attempts = excluded.attempts,
             launch_failures = excluded.launch_failures,
             last_attempt_at = excluded.last_attempt_at,
             stuck_ask_id = excluded.stuck_ask_id,
             silence_ask_id = excluded.silence_ask_id",
        rusqlite::params![
            row.task_id,
            row.agent,
            row.run_id,
            row.counted_run_id,
            row.attempts,
            row.launch_failures,
            row.last_attempt_at,
            row.stuck_ask_id,
            row.silence_ask_id,
        ],
    )
    .map_err(|e| sql_err("write a dispatch row to", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionStore;

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-dispatch-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        super::super::db::forget_connections();
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    #[test]
    fn an_untouched_task_has_no_row() {
        let store = scratch("absent");
        assert_eq!(store.dispatch_row("t1").expect("read"), None);
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_row_round_trips_and_an_upsert_replaces_it_whole() {
        let store = scratch("round-trip");
        let row = DispatchRow {
            task_id: "t1".into(),
            agent: "solver".into(),
            run_id: "r1".into(),
            counted_run_id: String::new(),
            attempts: 1,
            launch_failures: 0,
            last_attempt_at: 1_700,
            stuck_ask_id: String::new(),
            silence_ask_id: String::new(),
        };
        store.save_dispatch_row(&row).expect("save");
        assert_eq!(store.dispatch_row("t1").expect("read"), Some(row.clone()));

        let updated = DispatchRow {
            attempts: 2,
            counted_run_id: "r1".into(),
            ..row
        };
        store.save_dispatch_row(&updated).expect("save again");
        assert_eq!(store.dispatch_row("t1").expect("read"), Some(updated));

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
