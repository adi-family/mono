// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::{Error, Result};
use crate::migrations::migrations;
use crate::storage::{PendingRef, Storage, StructureRow};
use crate::structure::Structure;
use crate::types::{
    File, FileId, FileInfo, FileNode, Language, Location, Reference, ReferenceKind, Status, Symbol,
    SymbolId, SymbolKind, SymbolNode, SymbolUsage, Tree, Visibility,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The `symbols` columns [`SqliteStorage::row_to_symbol`] reads, in the order it indexes them.
///
/// Every read of a symbol row is positional, so this list and that function have to move
/// together: a column added to one and not the other does not fail, it silently shifts every
/// field after it. Naming the list once is what keeps them in step.
const SYMBOL_COLUMNS: &str = "id, name, kind, file_id, parent_id, start_line, start_col, \
     end_line, end_col, start_byte, end_byte, signature, description, doc_comment, visibility, \
     is_entry_point, structure_hash, structure_simhash, structure_size";

/// [`SYMBOL_COLUMNS`] qualified with a table alias, for the queries that join `files` — where
/// `id` and `description` are otherwise ambiguous.
fn symbol_columns_as(alias: &str) -> String {
    SYMBOL_COLUMNS
        .split(", ")
        .map(|column| format!("{alias}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read a [`Structure`] out of three consecutive columns starting at `first`.
///
/// All three are NULL together for a symbol stored before migration v3, and for one whose span
/// never resolved to a parse node — so anything short of all three present is `None` rather
/// than a partly-filled fingerprint that would compare as if it were real.
fn row_to_structure(row: &rusqlite::Row, first: usize) -> rusqlite::Result<Option<Structure>> {
    let hash: Option<String> = row.get(first)?;
    let simhash: Option<i64> = row.get(first + 1)?;
    let node_count: Option<u32> = row.get(first + 2)?;

    Ok(match (hash, simhash, node_count) {
        (Some(hash), Some(simhash), Some(node_count)) => Some(Structure {
            hash,
            // Written as a bit cast; see migration v3.
            simhash: simhash as u64,
            node_count,
        }),
        _ => None,
    })
}

/// Every row `sql` returns, decoded by `map`.
///
/// `map` reads its row positionally, so the indices it uses are the ones `sql` selects, in that
/// order. A row `map` rejects is skipped: one undecodable row costs the caller that row, not the
/// whole result.
///
/// The connection comes in already locked, because [`SqliteStorage::lock`] cannot be taken twice
/// — a caller that needs two queries under one guard passes the same one to both.
fn rows<T>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    map: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;

    let rows = stmt
        .query_map(params, map)?
        .filter_map(std::result::Result::ok)
        .collect();

    Ok(rows)
}

#[derive(Debug)]
pub struct SqliteStorage {
    conn: Mutex<Connection>,
}

impl SqliteStorage {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // WAL so a reader (a search) and the writer (an indexing run) don't block each other.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        Self::run_migrations(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// The connection, with a mutex poisoned by a panicking holder reported as [`Error::Storage`].
    ///
    /// A `std::sync::Mutex` is not reentrant, so this deadlocks against itself: a method that
    /// already holds the guard passes it down rather than locking a second time.
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|e| Error::Storage(e.to_string()))
    }

    fn run_migrations(conn: &Connection) -> Result<()> {
        let applied = crate::migrations::run(conn, &migrations())?;

        if applied > 0 {
            tracing::info!("Applied {applied} migration(s)");
        }

        Ok(())
    }

    fn row_to_file(&self, row: &rusqlite::Row) -> rusqlite::Result<File> {
        let lang_str: String = row.get(2)?;
        Ok(File {
            id: FileId(row.get(0)?),
            path: PathBuf::from(row.get::<_, String>(1)?),
            language: Language::parse(&lang_str),
            hash: row.get(3)?,
            size: row.get(4)?,
            description: row.get(5)?,
        })
    }

    fn row_to_symbol(&self, row: &rusqlite::Row, file_path: PathBuf) -> rusqlite::Result<Symbol> {
        let kind_str: String = row.get(2)?;
        let parent_id: Option<i64> = row.get(4)?;
        let visibility_str: String = row.get(14)?;
        let is_entry_point: i64 = row.get(15)?;
        Ok(Symbol {
            id: SymbolId(row.get(0)?),
            name: row.get(1)?,
            kind: SymbolKind::parse(&kind_str),
            file_id: FileId(row.get(3)?),
            file_path,
            parent_id: parent_id.map(SymbolId),
            location: Location {
                start_line: row.get(5)?,
                start_col: row.get(6)?,
                end_line: row.get(7)?,
                end_col: row.get(8)?,
                start_byte: row.get(9)?,
                end_byte: row.get(10)?,
            },
            signature: row.get(11)?,
            description: row.get(12)?,
            doc_comment: row.get(13)?,
            visibility: Visibility::parse(&visibility_str),
            is_entry_point: is_entry_point != 0,
            structure: row_to_structure(row, 16)?,
        })
    }

    /// A symbol from a row that selected [`SYMBOL_COLUMNS`] and then the file's path.
    ///
    /// The path lands in the column after the last of [`SYMBOL_COLUMNS`], so the index below
    /// moves whenever that list grows. It is spelled once here rather than in each query that
    /// joins `files`, for the same reason the list itself is named once.
    fn row_to_joined_symbol(&self, row: &rusqlite::Row) -> rusqlite::Result<Symbol> {
        self.row_to_symbol(row, PathBuf::from(row.get::<_, String>(19)?))
    }

    /// Find the innermost symbol whose `[start_line, end_line]` range
    /// covers `line` in `path`. Returns `(kind, name)` of the smallest
    /// match — so a `Method` inside a `Class` reports the method, an
    /// `Import` line reports `Import`, etc. None when the file isn't
    /// indexed or no symbol contains the line.
    ///
    /// `path` is matched verbatim against `files.path` — same form
    /// the indexer stored (relative to the indexed root).
    pub fn whois(&self, path: &str, line: u32) -> Result<Option<(SymbolKind, String)>> {
        let conn = self.lock()?;

        // Innermost match: smallest `(end_line - start_line)` span.
        // Tie-break on `start_line DESC` so a same-line symbol beats
        // its enclosing parent.
        let row = conn
            .query_row(
                "SELECT s.kind, s.name \
                 FROM symbols s JOIN files f ON f.id = s.file_id \
                 WHERE f.path = ?1 \
                   AND s.start_line <= ?2 \
                   AND s.end_line   >= ?2 \
                 ORDER BY (s.end_line - s.start_line) ASC, s.start_line DESC \
                 LIMIT 1",
                params![path, line],
                |row| {
                    let kind: String = row.get(0)?;
                    let name: String = row.get(1)?;
                    Ok((SymbolKind::parse(&kind), name))
                },
            )
            .ok();
        Ok(row)
    }

    /// The symbols on the other end of a `symbol_refs` edge from `id`.
    ///
    /// Callers and callees are the same query read in opposite directions: `matched` is the
    /// column `id` is looked up in, `joined` the one the symbol row is joined on. Callers pass
    /// `("to_symbol_id", "from_symbol_id")`, callees the swap.
    fn linked_symbols(&self, id: SymbolId, matched: &str, joined: &str) -> Result<Vec<Symbol>> {
        let conn = self.lock()?;

        rows(
            &conn,
            &format!(
                r"
            SELECT DISTINCT {COLS}, f.path
            FROM symbols s
            JOIN symbol_refs r ON r.{joined} = s.id
            JOIN files f ON f.id = s.file_id
            WHERE r.{matched} = ?1
            ",
                COLS = symbol_columns_as("s"),
            ),
            params![id.0],
            |row| self.row_to_joined_symbol(row),
        )
    }

    /// The `symbol_refs` rows whose `matched` column is `id` — `to_symbol_id` for the
    /// references *to* a symbol, `from_symbol_id` for the ones *from* it.
    fn references_on(&self, id: SymbolId, matched: &str) -> Result<Vec<Reference>> {
        let conn = self.lock()?;

        rows(
            &conn,
            &format!(
                "SELECT from_symbol_id, to_symbol_id, kind, start_line, start_col, end_line, \
                 end_col, start_byte, end_byte FROM symbol_refs WHERE {matched} = ?1",
            ),
            params![id.0],
            |row| {
                Ok(Reference {
                    from_symbol_id: SymbolId(row.get(0)?),
                    to_symbol_id: SymbolId(row.get(1)?),
                    kind: ReferenceKind::parse(&row.get::<_, String>(2)?),
                    location: Location {
                        start_line: row.get(3)?,
                        start_col: row.get(4)?,
                        end_line: row.get(5)?,
                        end_col: row.get(6)?,
                        start_byte: row.get(7)?,
                        end_byte: row.get(8)?,
                    },
                })
            },
        )
    }

    /// Run a bare transaction-control statement (`BEGIN TRANSACTION`, `COMMIT`, `ROLLBACK`).
    fn transaction_stmt(&self, sql: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(sql, [])?;
        Ok(())
    }
}

impl Storage for SqliteStorage {
    fn insert_file(&self, file: &File) -> Result<FileId> {
        let conn = self.lock()?;

        conn.execute(
            "INSERT INTO files (path, language, hash, size, description) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                file.path.to_string_lossy(),
                file.language.as_str(),
                file.hash,
                file.size,
                file.description
            ],
        )?;

        Ok(FileId(conn.last_insert_rowid()))
    }

    fn update_file(&self, file: &File) -> Result<()> {
        let conn = self.lock()?;

        conn.execute(
            "UPDATE files SET language = ?1, hash = ?2, size = ?3, description = ?4 WHERE id = ?5",
            params![
                file.language.as_str(),
                file.hash,
                file.size,
                file.description,
                file.id.0
            ],
        )?;

        Ok(())
    }

    fn delete_file(&self, path: &Path) -> Result<()> {
        let conn = self.lock()?;

        conn.execute(
            "DELETE FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
        )?;

        Ok(())
    }

    fn get_file(&self, path: &Path) -> Result<FileInfo> {
        let conn = self.lock()?;

        let file: File = conn
            .query_row(
                "SELECT id, path, language, hash, size, description FROM files WHERE path = ?1",
                params![path.to_string_lossy()],
                |row| self.row_to_file(row),
            )
            .map_err(|_| Error::NotFound(format!("File not found: {}", path.display())))?;

        let file_path = file.path.clone();
        let file_id = file.id;

        let symbols = rows(
            &conn,
            &format!("SELECT {SYMBOL_COLUMNS} FROM symbols WHERE file_id = ?1"),
            params![file_id.0],
            |row| self.row_to_symbol(row, file_path.clone()),
        )?;

        Ok(FileInfo { file, symbols })
    }

    fn get_file_by_id(&self, id: FileId) -> Result<File> {
        let conn = self.lock()?;

        conn.query_row(
            "SELECT id, path, language, hash, size, description FROM files WHERE id = ?1",
            params![id.0],
            |row| self.row_to_file(row),
        )
        .map_err(|_| Error::NotFound(format!("File not found: {id:?}")))
    }

    fn file_exists(&self, path: &Path) -> Result<bool> {
        let conn = self.lock()?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    fn get_file_hash(&self, path: &Path) -> Result<Option<String>> {
        let conn = self.lock()?;

        conn.query_row(
            "SELECT hash FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| Error::Storage(e.to_string()))
    }

    fn insert_symbol(&self, symbol: &Symbol) -> Result<SymbolId> {
        let conn = self.lock()?;

        conn.execute(
            "INSERT INTO symbols (name, kind, file_id, parent_id, start_line, start_col, end_line, end_col, start_byte, end_byte, signature, description, doc_comment, visibility, is_entry_point, structure_hash, structure_simhash, structure_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                symbol.name,
                symbol.kind.as_str(),
                symbol.file_id.0,
                symbol.parent_id.map(|id| id.0),
                symbol.location.start_line,
                symbol.location.start_col,
                symbol.location.end_line,
                symbol.location.end_col,
                symbol.location.start_byte,
                symbol.location.end_byte,
                symbol.signature,
                symbol.description,
                symbol.doc_comment,
                symbol.visibility.as_str(),
                i64::from(symbol.is_entry_point),
                symbol.structure.as_ref().map(|s| s.hash.as_str()),
                symbol.structure.as_ref().map(|s| s.simhash as i64),
                symbol.structure.as_ref().map(|s| s.node_count)
            ],
        )?;

        Ok(SymbolId(conn.last_insert_rowid()))
    }

    fn update_symbol(&self, symbol: &Symbol) -> Result<()> {
        let conn = self.lock()?;

        conn.execute(
            "UPDATE symbols SET name = ?1, kind = ?2, start_line = ?3, start_col = ?4, end_line = ?5, end_col = ?6, start_byte = ?7, end_byte = ?8, signature = ?9, description = ?10, doc_comment = ?11, visibility = ?12, is_entry_point = ?13, structure_hash = ?14, structure_simhash = ?15, structure_size = ?16 WHERE id = ?17",
            params![
                symbol.name,
                symbol.kind.as_str(),
                symbol.location.start_line,
                symbol.location.start_col,
                symbol.location.end_line,
                symbol.location.end_col,
                symbol.location.start_byte,
                symbol.location.end_byte,
                symbol.signature,
                symbol.description,
                symbol.doc_comment,
                symbol.visibility.as_str(),
                i64::from(symbol.is_entry_point),
                symbol.structure.as_ref().map(|s| s.hash.as_str()),
                symbol.structure.as_ref().map(|s| s.simhash as i64),
                symbol.structure.as_ref().map(|s| s.node_count),
                symbol.id.0
            ],
        )?;

        Ok(())
    }

    fn delete_symbols_for_file(&self, file_id: FileId) -> Result<()> {
        let conn = self.lock()?;

        conn.execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id.0])?;

        Ok(())
    }

    fn get_symbol(&self, id: SymbolId) -> Result<Symbol> {
        let conn = self.lock()?;

        // First get the file path
        let file_path: PathBuf = conn
            .query_row(
                "SELECT f.path FROM files f JOIN symbols s ON f.id = s.file_id WHERE s.id = ?1",
                params![id.0],
                |row| Ok(PathBuf::from(row.get::<_, String>(0)?)),
            )
            .map_err(|_| Error::NotFound(format!("Symbol not found: {id:?}")))?;

        conn.query_row(
            &format!("SELECT {SYMBOL_COLUMNS} FROM symbols WHERE id = ?1"),
            params![id.0],
            |row| self.row_to_symbol(row, file_path),
        )
        .map_err(|_| Error::NotFound(format!("Symbol not found: {id:?}")))
    }

    fn get_symbols_for_file(&self, file_id: FileId) -> Result<Vec<Symbol>> {
        let conn = self.lock()?;

        let file_path: PathBuf = conn
            .query_row(
                "SELECT path FROM files WHERE id = ?1",
                params![file_id.0],
                |row| Ok(PathBuf::from(row.get::<_, String>(0)?)),
            )
            .map_err(|_| Error::NotFound(format!("File not found: {file_id:?}")))?;

        rows(
            &conn,
            &format!("SELECT {SYMBOL_COLUMNS} FROM symbols WHERE file_id = ?1"),
            params![file_id.0],
            |row| self.row_to_symbol(row, file_path.clone()),
        )
    }

    fn get_all_symbols(&self) -> Result<Vec<Symbol>> {
        let conn = self.lock()?;

        rows(
            &conn,
            &format!(
                "SELECT {}, f.path FROM symbols s JOIN files f ON s.file_id = f.id",
                symbol_columns_as("s")
            ),
            params![],
            |row| self.row_to_joined_symbol(row),
        )
    }

    fn insert_reference(&self, reference: &Reference) -> Result<()> {
        let conn = self.lock()?;

        conn.execute(
            "INSERT OR IGNORE INTO symbol_refs (from_symbol_id, to_symbol_id, kind, start_line, start_col, end_line, end_col, start_byte, end_byte) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                reference.from_symbol_id.0,
                reference.to_symbol_id.0,
                reference.kind.as_str(),
                reference.location.start_line,
                reference.location.start_col,
                reference.location.end_line,
                reference.location.end_col,
                reference.location.start_byte,
                reference.location.end_byte,
            ],
        )?;

        Ok(())
    }

    fn insert_references_batch(&self, references: &[Reference]) -> Result<()> {
        let conn = self.lock()?;

        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO symbol_refs (from_symbol_id, to_symbol_id, kind, start_line, start_col, end_line, end_col, start_byte, end_byte) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;

        for reference in references {
            stmt.execute(params![
                reference.from_symbol_id.0,
                reference.to_symbol_id.0,
                reference.kind.as_str(),
                reference.location.start_line,
                reference.location.start_col,
                reference.location.end_line,
                reference.location.end_col,
                reference.location.start_byte,
                reference.location.end_byte,
            ])?;
        }

        Ok(())
    }

    fn replace_pending_refs(&self, file_id: FileId, refs: &[PendingRef]) -> Result<()> {
        let conn = self.lock()?;

        conn.execute(
            "DELETE FROM pending_refs WHERE from_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![file_id.0],
        )?;

        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO pending_refs (from_symbol_id, target_name, kind, start_line, start_col, end_line, end_col, start_byte, end_byte) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;

        for reference in refs {
            stmt.execute(params![
                reference.from_symbol_id.0,
                reference.target_name,
                reference.kind.as_str(),
                reference.location.start_line,
                reference.location.start_col,
                reference.location.end_line,
                reference.location.end_col,
                reference.location.start_byte,
                reference.location.end_byte,
            ])?;
        }

        Ok(())
    }

    fn all_pending_refs(&self) -> Result<Vec<PendingRef>> {
        let conn = self.lock()?;

        rows(
            &conn,
            "SELECT from_symbol_id, target_name, kind, start_line, start_col, end_line, end_col, \
             start_byte, end_byte FROM pending_refs",
            [],
            |row| {
                Ok(PendingRef {
                    from_symbol_id: SymbolId(row.get(0)?),
                    target_name: row.get(1)?,
                    kind: ReferenceKind::parse(&row.get::<_, String>(2)?),
                    location: Location {
                        start_line: row.get(3)?,
                        start_col: row.get(4)?,
                        end_line: row.get(5)?,
                        end_col: row.get(6)?,
                        start_byte: row.get(7)?,
                        end_byte: row.get(8)?,
                    },
                })
            },
        )
    }

    fn replace_symbol_refs(&self, refs: &[Reference]) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM symbol_refs", [])?;
        drop(conn);

        self.insert_references_batch(refs)
    }

    fn delete_references_for_file(&self, file_id: FileId) -> Result<()> {
        let conn = self.lock()?;

        conn.execute(
            "DELETE FROM symbol_refs WHERE from_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1) \
             OR to_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![file_id.0],
        )?;

        Ok(())
    }

    fn get_callers(&self, id: SymbolId) -> Result<Vec<Symbol>> {
        self.linked_symbols(id, "to_symbol_id", "from_symbol_id")
    }

    fn get_callees(&self, id: SymbolId) -> Result<Vec<Symbol>> {
        self.linked_symbols(id, "from_symbol_id", "to_symbol_id")
    }

    fn get_reference_count(&self, id: SymbolId) -> Result<u64> {
        let conn = self.lock()?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM symbol_refs WHERE to_symbol_id = ?1",
            params![id.0],
            |row| row.get(0),
        )?;

        Ok(count as u64)
    }

    fn get_references_to(&self, id: SymbolId) -> Result<Vec<Reference>> {
        self.references_on(id, "to_symbol_id")
    }

    fn get_references_from(&self, id: SymbolId) -> Result<Vec<Reference>> {
        self.references_on(id, "from_symbol_id")
    }

    fn find_symbols_by_name(&self, name: &str) -> Result<Vec<Symbol>> {
        let conn = self.lock()?;

        rows(
            &conn,
            &format!(
                r"
            SELECT {COLS}, f.path
            FROM symbols s
            JOIN files f ON f.id = s.file_id
            WHERE s.name = ?1
            ",
                COLS = symbol_columns_as("s"),
            ),
            params![name],
            |row| self.row_to_joined_symbol(row),
        )
    }

    fn get_symbol_usage(&self, id: SymbolId) -> Result<SymbolUsage> {
        let symbol = self.get_symbol(id)?;
        let reference_count = self.get_reference_count(id)?;
        let callers = self.get_callers(id)?;
        let callees = self.get_callees(id)?;

        Ok(SymbolUsage {
            symbol,
            reference_count,
            callers,
            callees,
        })
    }

    fn search_symbols_fts(&self, query: &str, limit: usize) -> Result<Vec<Symbol>> {
        let conn = self.lock()?;

        rows(
            &conn,
            &format!(
                r"
            SELECT {COLS}, f.path
            FROM symbols s
            JOIN symbols_fts fts ON fts.rowid = s.id
            JOIN files f ON f.id = s.file_id
            WHERE symbols_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            ",
                COLS = symbol_columns_as("s"),
            ),
            params![query, limit as i64],
            |row| self.row_to_joined_symbol(row),
        )
    }

    fn search_files_fts(&self, query: &str, limit: usize) -> Result<Vec<File>> {
        let conn = self.lock()?;

        rows(
            &conn,
            r"
            SELECT f.id, f.path, f.language, f.hash, f.size, f.description
            FROM files f
            JOIN files_fts fts ON fts.rowid = f.id
            WHERE files_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            ",
            params![query, limit as i64],
            |row| self.row_to_file(row),
        )
    }

    fn structures(&self, min_nodes: u32) -> Result<Vec<StructureRow>> {
        let conn = self.lock()?;

        rows(
            &conn,
            r"
            SELECT s.id, s.name, s.kind, f.path, s.start_line, s.end_line,
                   s.structure_hash, s.structure_simhash, s.structure_size
            FROM symbols s
            JOIN files f ON f.id = s.file_id
            WHERE s.structure_hash IS NOT NULL AND s.structure_size >= ?1
            ",
            params![min_nodes],
            |row| {
                let kind: String = row.get(2)?;
                Ok(StructureRow {
                    id: SymbolId(row.get(0)?),
                    name: row.get(1)?,
                    kind: SymbolKind::parse(&kind),
                    file_path: PathBuf::from(row.get::<_, String>(3)?),
                    start_line: row.get(4)?,
                    end_line: row.get(5)?,
                    // The WHERE clause guarantees the columns are present; a row that somehow
                    // is not gets dropped by `rows` rather than faked into a zero fingerprint.
                    structure: row_to_structure(row, 6)?.ok_or(
                        rusqlite::Error::InvalidColumnType(
                            6,
                            "structure_hash".to_string(),
                            rusqlite::types::Type::Null,
                        ),
                    )?,
                })
            },
        )
    }

    fn get_tree(&self) -> Result<Tree> {
        let conn = self.lock()?;

        let files: Vec<(FileId, PathBuf, Language)> = rows(
            &conn,
            "SELECT id, path, language, hash, size, description FROM files ORDER BY path",
            params![],
            |row| {
                let lang_str: String = row.get(2)?;
                Ok((
                    FileId(row.get(0)?),
                    PathBuf::from(row.get::<_, String>(1)?),
                    Language::parse(&lang_str),
                ))
            },
        )?;

        let mut nodes = Vec::new();

        for (file_id, path, language) in files {
            let symbols: Vec<(i64, String, String, Option<i64>)> = rows(
                &conn,
                "SELECT id, name, kind, parent_id FROM symbols WHERE file_id = ?1 ORDER BY start_line",
                params![file_id.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;

            // Build tree structure
            let symbol_nodes: Vec<SymbolNode> = symbols
                .iter()
                .filter(|(_, _, _, parent)| parent.is_none())
                .map(|(id, name, kind, _)| SymbolNode {
                    id: SymbolId(*id),
                    name: name.clone(),
                    kind: SymbolKind::parse(kind),
                    children: vec![],
                })
                .collect();

            nodes.push(FileNode {
                path,
                language,
                symbols: symbol_nodes,
            });
        }

        Ok(Tree { files: nodes })
    }

    fn get_status(&self) -> Result<Status> {
        let conn = self.lock()?;

        let indexed_files: i64 =
            conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;

        let indexed_symbols: i64 =
            conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;

        let get_status_value = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM status WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
        };

        // Ask SQLite for its own size rather than stat-ing the file: with WAL on, pages that
        // are written but not yet checkpointed live in `-wal`, and the main file understates.
        let storage_size_bytes: i64 = conn
            .query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(Status {
            indexed_files: indexed_files as u64,
            indexed_symbols: indexed_symbols as u64,
            embedding_dimensions: get_status_value("embedding_dimensions")
                .and_then(|s| s.parse().ok())
                .unwrap_or(768),
            embedding_model: get_status_value("embedding_model")
                .unwrap_or_else(|| "jinaai/jina-embeddings-v2-base-code".to_string()),
            last_indexed: get_status_value("last_indexed"),
            storage_size_bytes: storage_size_bytes.max(0) as u64,
            pipeline_version: get_status_value("pipeline_version")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        })
    }

    fn update_status(&self, status: &Status) -> Result<()> {
        let conn = self.lock()?;

        conn.execute(
            "INSERT OR REPLACE INTO status (key, value) VALUES ('embedding_dimensions', ?1)",
            params![status.embedding_dimensions.to_string()],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO status (key, value) VALUES ('embedding_model', ?1)",
            params![status.embedding_model],
        )?;

        if let Some(ref last_indexed) = status.last_indexed {
            conn.execute(
                "INSERT OR REPLACE INTO status (key, value) VALUES ('last_indexed', ?1)",
                params![last_indexed],
            )?;
        }

        conn.execute(
            "INSERT OR REPLACE INTO status (key, value) VALUES ('pipeline_version', ?1)",
            params![status.pipeline_version.to_string()],
        )?;

        Ok(())
    }

    fn begin_transaction(&self) -> Result<()> {
        self.transaction_stmt("BEGIN TRANSACTION")
    }

    fn commit_transaction(&self) -> Result<()> {
        self.transaction_stmt("COMMIT")
    }

    fn rollback_transaction(&self) -> Result<()> {
        self.transaction_stmt("ROLLBACK")
    }

    fn indexed_files(&self) -> Result<Vec<(FileId, PathBuf)>> {
        let conn = self.lock()?;

        rows(&conn, "SELECT id, path FROM files", [], |row| {
            Ok((FileId(row.get(0)?), PathBuf::from(row.get::<_, String>(1)?)))
        })
    }

    fn delete_file_cascade(&self, id: FileId) -> Result<Vec<SymbolId>> {
        let conn = self.lock()?;

        let symbols = rows(
            &conn,
            "SELECT id FROM symbols WHERE file_id = ?1",
            params![id.0],
            |row| Ok(SymbolId(row.get(0)?)),
        )?;

        // Nothing turns `PRAGMA foreign_keys` on, so the schema's ON DELETE CASCADE never runs
        // and every dependent row has to be named here. The FTS tables are the exception — they
        // are kept in step by triggers, which do fire on the deletes below.
        conn.execute(
            "DELETE FROM symbol_refs WHERE from_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1) \
             OR to_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![id.0],
        )?;
        conn.execute(
            "DELETE FROM reachability_cache WHERE symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![id.0],
        )?;
        conn.execute("DELETE FROM symbols WHERE file_id = ?1", params![id.0])?;
        conn.execute("DELETE FROM files WHERE id = ?1", params![id.0])?;

        Ok(symbols)
    }
}
