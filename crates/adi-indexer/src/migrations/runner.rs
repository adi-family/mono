//! Forward-only SQLite migrations.
//!
//! Applied versions are recorded in `schema_migrations`; each migration runs inside its own
//! transaction, so a failure leaves the database at the last version that fully applied.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// One migration: a version, a name for the log, and the SQL that takes the schema forward.
#[derive(Debug, Clone)]
pub struct SqlMigration {
    pub version: i64,
    pub name: String,
    pub up_sql: String,
}

impl SqlMigration {
    pub fn new(version: i64, name: impl Into<String>, up_sql: impl Into<String>) -> Self {
        Self {
            version,
            name: name.into(),
            up_sql: up_sql.into(),
        }
    }
}

/// Apply every migration the database has not seen, in version order. Returns how many ran.
pub fn run(conn: &Connection, migrations: &[SqlMigration]) -> Result<usize> {
    validate(migrations)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
        [],
    )?;

    let current: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .unwrap_or(None)
        .unwrap_or(0);

    let mut pending: Vec<&SqlMigration> = migrations
        .iter()
        .filter(|m| m.version > current)
        .collect();
    pending.sort_by_key(|m| m.version);

    for migration in &pending {
        apply(conn, migration)?;
        tracing::debug!(
            version = migration.version,
            name = migration.name,
            "applied migration"
        );
    }

    Ok(pending.len())
}

fn apply(conn: &Connection, migration: &SqlMigration) -> Result<()> {
    conn.execute_batch("BEGIN")?;

    let applied = conn
        .execute_batch(&migration.up_sql)
        .map_err(Error::from)
        .and_then(|()| {
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                (migration.version, now_unix()),
            )
            .map_err(Error::from)
        });

    match applied {
        Ok(_) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(Error::Storage(format!(
                "migration {} ({}) failed: {e}",
                migration.version, migration.name
            )))
        }
    }
}

/// Versions must be exactly 1..=n: a gap or a duplicate means two branches picked the same
/// number, and applying them in that state would leave databases with different schemas at the
/// same recorded version.
fn validate(migrations: &[SqlMigration]) -> Result<()> {
    let mut versions: Vec<i64> = migrations.iter().map(|m| m.version).collect();
    versions.sort_unstable();

    for (i, version) in versions.iter().enumerate() {
        let expected = i as i64 + 1;
        if *version != expected {
            return Err(Error::Storage(format!(
                "migrations are out of order: expected version {expected}, found {version}"
            )));
        }
    }

    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> Vec<SqlMigration> {
        vec![
            SqlMigration::new(1, "users", "CREATE TABLE users (id INTEGER PRIMARY KEY)"),
            SqlMigration::new(2, "orders", "CREATE TABLE orders (id INTEGER PRIMARY KEY)"),
        ]
    }

    #[test]
    fn applies_once_and_then_stops() {
        let conn = Connection::open_in_memory().unwrap();

        assert_eq!(run(&conn, &set()).unwrap(), 2);
        assert_eq!(run(&conn, &set()).unwrap(), 0);

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('users','orders')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2);
    }

    #[test]
    fn picks_up_a_migration_added_later() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, &set()[..1]).unwrap();

        assert_eq!(run(&conn, &set()).unwrap(), 1);
    }

    #[test]
    fn a_gap_in_versions_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        let gapped = vec![
            SqlMigration::new(1, "first", "SELECT 1"),
            SqlMigration::new(3, "third", "SELECT 1"),
        ];

        assert!(run(&conn, &gapped).is_err());
    }

    #[test]
    fn a_failed_migration_is_not_recorded() {
        let conn = Connection::open_in_memory().unwrap();
        let broken = vec![SqlMigration::new(1, "broken", "CREATE TABLE (")];

        assert!(run(&conn, &broken).is_err());

        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 0);
    }
}
