//! Rekey an idle agent's history without separating its rows from its files.

use std::path::{Path, PathBuf};

use rusqlite::{Transaction, TransactionBehavior};

use super::{SessionStore, db::sql_err};
use crate::error::{Error, Result};

pub(super) fn agent(
    store: &SessionStore,
    from: &str,
    to: &str,
    manifest_from: &Path,
    manifest_to: &Path,
) -> Result<()> {
    let conn = store.conn()?;
    let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
        .map_err(|e| sql_err("rename an agent in", e))?;
    let occupied: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE agent = ?1)",
            [to],
            |row| row.get(0),
        )
        .map_err(|e| sql_err("check an agent rename in", e))?;
    let old_dir = store.agent_dir(from);
    let new_dir = store.agent_dir(to);
    // A deleted agent can still own history. Never merge it with a different definition, or
    // replace an existing directory just because there is no manifest at the destination.
    if occupied || new_dir.try_exists()? || manifest_to.try_exists()? {
        return Err(Error::Exists(to.to_string()));
    }

    // Existing databases have ON DELETE CASCADE but no ON UPDATE action. Defer the foreign-key
    // checks until every table has the new owner; no schema rewrite or disabled checks is needed.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON")
        .map_err(|e| sql_err("defer rename constraints in", e))?;
    for table in [
        "sessions",
        "turns",
        "queue",
        "questions",
        "goals",
        "attachments",
    ] {
        tx.execute(
            &format!("UPDATE {table} SET agent = ?2 WHERE agent = ?1"),
            [from, to],
        )
        .map_err(|e| sql_err("rename session ownership in", e))?;
    }
    // Questions also embed their owner in their JSON payload. Patch only that field so future
    // fields and all recorded answers survive the move.
    tx.execute(
        "UPDATE questions SET json = json_set(json, '$.agent', ?1) WHERE agent = ?1",
        [to],
    )
    .map_err(|e| sql_err("rename question ownership in", e))?;

    let mut moved = Vec::new();
    let result = (|| {
        if old_dir.try_exists()? {
            move_path(&old_dir, &new_dir, &mut moved)?;
        }
        move_path(manifest_from, manifest_to, &mut moved)?;
        tx.commit()
            .map_err(|e| sql_err("commit an agent rename in", e))
    })();
    if let Err(error) = result {
        // SQL rolls back on drop. Restore file moves in reverse order before returning the error.
        for (old, new) in moved.iter().rev() {
            if let Err(restore) = std::fs::rename(new, old) {
                return Err(Error::Session(format!(
                    "{error}; could not restore {} to {}: {restore}",
                    new.display(),
                    old.display()
                )));
            }
        }
        return Err(error);
    }
    Ok(())
}

fn move_path(from: &Path, to: &Path, moved: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    std::fs::rename(from, to)?;
    moved.push((from.to_path_buf(), to.to_path_buf()));
    Ok(())
}
