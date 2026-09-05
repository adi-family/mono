//! Fleet activity: durable last-seen bookkeeping for paired peers.
//!
//! Kept in the platform's shared global database (`guides/db.md`) rather than anywhere private to
//! this process, because presence is a question more than this process ever has to answer — the
//! control panel's `/api/*` handlers want it too (`docs/fleet.md`'s presence work), and a second,
//! in-process copy of "who was last seen when" is a second answer that can disagree with this one.
//! `fleet_activity` is a small, self-contained table: one row per node nickname, so any reader of
//! `db/global.db` sees exactly what [`record_seen`] last wrote, with no channel of its own to keep
//! in sync.
//!
//! Written from exactly one place: [`crate::gateway::negotiate`], once a peer's request has
//! cleared both gates (admitted, authenticated) — the same moment its identity is attached to the
//! request as [`crate::auth::FLEET_NODE_HEADER`], so "last seen" always means "seen as this node".

use adi_db::Db;
use tracing::debug;

/// The table this module owns. `if not exists` because whichever process reaches it first is the
/// one that creates it — there is no migration to run and no reason to prefer one writer over
/// another. `node` is the primary key: one row per nickname, always the most recent sighting.
const SCHEMA: &str = "\
    create table if not exists fleet_activity (
        node      text primary key,
        last_seen integer not null
    )";

/// Record that `nickname` — a paired peer whose request just cleared both gates — was seen now.
///
/// Best-effort and fire-and-forget: a write that fails (a locked file, an unreadable store) is
/// logged at debug and dropped rather than propagated. Presence is a nicety layered on the mesh,
/// not a condition of it — a proxied request must never fail because its bookkeeping didn't fit
/// through a busy database.
pub fn record_seen(nickname: &str) {
    if let Err(e) = try_record_seen(&Db::open(), nickname, adi_config::now_unix()) {
        debug!(%nickname, error = %e, "gateway: could not record fleet activity");
    }
}

/// The pure half of [`record_seen`]: the store and the clock passed in, so the write is testable
/// against a temp store and a fixed time rather than the real one and [`std::time::SystemTime`].
fn try_record_seen(db: &Db, nickname: &str, now: u64) -> adi_db::Result<()> {
    db.exec(None, SCHEMA, &[])?;
    db.exec(
        None,
        "insert into fleet_activity (node, last_seen) values (?1, ?2)
         on conflict(node) do update set last_seen = excluded.last_seen",
        &[nickname.to_string(), now.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store rooted in a temp dir, so no test ever touches the operator's real `db/global.db`.
    fn scratch(tag: &str) -> Db {
        let root = std::env::temp_dir().join(format!(
            "adi-mesh-activity-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Db::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn a_sighting_is_recorded_and_a_later_one_updates_it_in_place() {
        let db = scratch("basic");
        try_record_seen(&db, "laptop-b", 1_700_000_000).expect("first write");
        let rows = db
            .query(None, "select node, last_seen from fleet_activity", &[])
            .expect("query");
        assert_eq!(
            rows.rows,
            vec![vec![
                serde_json::json!("laptop-b"),
                serde_json::json!(1_700_000_000)
            ]]
        );

        // The same node seen again updates the row rather than adding a second one.
        try_record_seen(&db, "laptop-b", 1_700_000_050).expect("second write");
        let rows = db
            .query(None, "select node, last_seen from fleet_activity", &[])
            .expect("query");
        assert_eq!(
            rows.rows,
            vec![vec![
                serde_json::json!("laptop-b"),
                serde_json::json!(1_700_000_050)
            ]],
            "one row per node, holding the most recent sighting"
        );
    }

    #[test]
    fn different_nodes_are_tracked_independently() {
        let db = scratch("two-nodes");
        try_record_seen(&db, "laptop-a", 100).expect("a");
        try_record_seen(&db, "laptop-b", 200).expect("b");
        let rows = db
            .query(
                None,
                "select node, last_seen from fleet_activity order by node",
                &[],
            )
            .expect("query");
        assert_eq!(
            rows.rows,
            vec![
                vec![serde_json::json!("laptop-a"), serde_json::json!(100)],
                vec![serde_json::json!("laptop-b"), serde_json::json!(200)],
            ]
        );
    }

    /// [`record_seen`] itself must never panic even when it cannot reach a database — the caller is
    /// a request on its way to a service, and bookkeeping is not allowed to be the reason it fails.
    #[test]
    fn record_seen_does_not_panic_against_an_unwritable_store() {
        let root = std::env::temp_dir().join(format!(
            "adi-mesh-activity-unwritable-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch dir");
        // A file where the `db` directory should be: every attempt to create it underneath fails.
        std::fs::write(root.join("db"), b"not a directory").expect("blocker file");
        let db = Db::with_config(adi_config::Config::with_root(&root));
        assert!(
            try_record_seen(&db, "laptop-b", 1).is_err(),
            "the store refused the write"
        );
    }
}
