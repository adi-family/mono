// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::{Error, Result};
use crate::migrations::migrations;
use crate::storage::Storage;
use crate::types::{File, FileId, Language, Symbol, SymbolId, SymbolKind, Location, Visibility, FileInfo, Reference, ReferenceKind, SymbolUsage, Tree, SymbolNode, FileNode, Status};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
        })
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

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
}

impl Storage for SqliteStorage {
    fn insert_file(&self, file: &File) -> Result<FileId> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        conn.execute(
            "DELETE FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
        )?;

        Ok(())
    }

    fn get_file(&self, path: &Path) -> Result<FileInfo> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let file: File = conn
            .query_row(
                "SELECT id, path, language, hash, size, description FROM files WHERE path = ?1",
                params![path.to_string_lossy()],
                |row| self.row_to_file(row),
            )
            .map_err(|_| Error::NotFound(format!("File not found: {}", path.display())))?;

        let file_path = file.path.clone();
        let file_id = file.id;

        let mut stmt = conn.prepare(
            "SELECT id, name, kind, file_id, parent_id, start_line, start_col, end_line, end_col, start_byte, end_byte, signature, description, doc_comment, visibility, is_entry_point FROM symbols WHERE file_id = ?1",
        )?;

        let symbols: Vec<Symbol> = stmt
            .query_map(params![file_id.0], |row| {
                self.row_to_symbol(row, file_path.clone())
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(FileInfo { file, symbols })
    }

    fn get_file_by_id(&self, id: FileId) -> Result<File> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        conn.query_row(
            "SELECT id, path, language, hash, size, description FROM files WHERE id = ?1",
            params![id.0],
            |row| self.row_to_file(row),
        )
        .map_err(|_| Error::NotFound(format!("File not found: {id:?}")))
    }

    fn file_exists(&self, path: &Path) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    fn get_file_hash(&self, path: &Path) -> Result<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        conn.query_row(
            "SELECT hash FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| Error::Storage(e.to_string()))
    }

    fn insert_symbol(&self, symbol: &Symbol) -> Result<SymbolId> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        conn.execute(
            "INSERT INTO symbols (name, kind, file_id, parent_id, start_line, start_col, end_line, end_col, start_byte, end_byte, signature, description, doc_comment, visibility, is_entry_point) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                i64::from(symbol.is_entry_point)
            ],
        )?;

        Ok(SymbolId(conn.last_insert_rowid()))
    }

    fn update_symbol(&self, symbol: &Symbol) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        conn.execute(
            "UPDATE symbols SET name = ?1, kind = ?2, start_line = ?3, start_col = ?4, end_line = ?5, end_col = ?6, start_byte = ?7, end_byte = ?8, signature = ?9, description = ?10, doc_comment = ?11, visibility = ?12, is_entry_point = ?13 WHERE id = ?14",
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
                symbol.id.0
            ],
        )?;

        Ok(())
    }

    fn delete_symbols_for_file(&self, file_id: FileId) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        conn.execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id.0])?;

        Ok(())
    }

    fn get_symbol(&self, id: SymbolId) -> Result<Symbol> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        // First get the file path
        let file_path: PathBuf = conn
            .query_row(
                "SELECT f.path FROM files f JOIN symbols s ON f.id = s.file_id WHERE s.id = ?1",
                params![id.0],
                |row| Ok(PathBuf::from(row.get::<_, String>(0)?)),
            )
            .map_err(|_| Error::NotFound(format!("Symbol not found: {id:?}")))?;

        conn.query_row(
            "SELECT id, name, kind, file_id, parent_id, start_line, start_col, end_line, end_col, start_byte, end_byte, signature, description, doc_comment, visibility, is_entry_point FROM symbols WHERE id = ?1",
            params![id.0],
            |row| self.row_to_symbol(row, file_path),
        )
        .map_err(|_| Error::NotFound(format!("Symbol not found: {id:?}")))
    }

    fn get_symbols_for_file(&self, file_id: FileId) -> Result<Vec<Symbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let file_path: PathBuf = conn
            .query_row(
                "SELECT path FROM files WHERE id = ?1",
                params![file_id.0],
                |row| Ok(PathBuf::from(row.get::<_, String>(0)?)),
            )
            .map_err(|_| Error::NotFound(format!("File not found: {file_id:?}")))?;

        let mut stmt = conn.prepare(
            "SELECT id, name, kind, file_id, parent_id, start_line, start_col, end_line, end_col, start_byte, end_byte, signature, description, doc_comment, visibility, is_entry_point FROM symbols WHERE file_id = ?1",
        )?;

        let symbols: Vec<Symbol> = stmt
            .query_map(params![file_id.0], |row| {
                self.row_to_symbol(row, file_path.clone())
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(symbols)
    }

    fn get_all_symbols(&self) -> Result<Vec<Symbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.kind, s.file_id, s.parent_id, s.start_line, s.start_col, s.end_line, s.end_col, s.start_byte, s.end_byte, s.signature, s.description, s.doc_comment, s.visibility, s.is_entry_point, f.path FROM symbols s JOIN files f ON s.file_id = f.id",
        )?;

        let symbols: Vec<Symbol> = stmt
            .query_map([], |row| {
                let file_path = PathBuf::from(row.get::<_, String>(16)?);
                self.row_to_symbol(row, file_path)
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(symbols)
    }

    fn insert_reference(&self, reference: &Reference) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

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

    fn delete_references_for_file(&self, file_id: FileId) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        // Delete references where the from_symbol belongs to this file
        conn.execute(
            "DELETE FROM symbol_refs WHERE from_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![file_id.0],
        )?;

        Ok(())
    }

    fn get_callers(&self, id: SymbolId) -> Result<Vec<Symbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT DISTINCT s.id, s.name, s.kind, s.file_id, s.parent_id, s.start_line, s.start_col, s.end_line, s.end_col, s.start_byte, s.end_byte, s.signature, s.description, s.doc_comment, f.path
            FROM symbols s
            JOIN symbol_refs r ON r.from_symbol_id = s.id
            JOIN files f ON f.id = s.file_id
            WHERE r.to_symbol_id = ?1
            ",
        )?;

        let symbols: Vec<Symbol> = stmt
            .query_map(params![id.0], |row| {
                let file_path = PathBuf::from(row.get::<_, String>(14)?);
                self.row_to_symbol(row, file_path)
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(symbols)
    }

    fn get_callees(&self, id: SymbolId) -> Result<Vec<Symbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT DISTINCT s.id, s.name, s.kind, s.file_id, s.parent_id, s.start_line, s.start_col, s.end_line, s.end_col, s.start_byte, s.end_byte, s.signature, s.description, s.doc_comment, f.path
            FROM symbols s
            JOIN symbol_refs r ON r.to_symbol_id = s.id
            JOIN files f ON f.id = s.file_id
            WHERE r.from_symbol_id = ?1
            ",
        )?;

        let symbols: Vec<Symbol> = stmt
            .query_map(params![id.0], |row| {
                let file_path = PathBuf::from(row.get::<_, String>(14)?);
                self.row_to_symbol(row, file_path)
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(symbols)
    }

    fn get_reference_count(&self, id: SymbolId) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM symbol_refs WHERE to_symbol_id = ?1",
            params![id.0],
            |row| row.get(0),
        )?;

        Ok(count as u64)
    }

    fn get_references_to(&self, id: SymbolId) -> Result<Vec<Reference>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            "SELECT from_symbol_id, to_symbol_id, kind, start_line, start_col, end_line, end_col, start_byte, end_byte FROM symbol_refs WHERE to_symbol_id = ?1",
        )?;

        let refs: Vec<Reference> = stmt
            .query_map(params![id.0], |row| {
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
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(refs)
    }

    fn get_references_from(&self, id: SymbolId) -> Result<Vec<Reference>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            "SELECT from_symbol_id, to_symbol_id, kind, start_line, start_col, end_line, end_col, start_byte, end_byte FROM symbol_refs WHERE from_symbol_id = ?1",
        )?;

        let refs: Vec<Reference> = stmt
            .query_map(params![id.0], |row| {
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
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(refs)
    }

    fn find_symbols_by_name(&self, name: &str) -> Result<Vec<Symbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT s.id, s.name, s.kind, s.file_id, s.parent_id, s.start_line, s.start_col, s.end_line, s.end_col, s.start_byte, s.end_byte, s.signature, s.description, s.doc_comment, f.path
            FROM symbols s
            JOIN files f ON f.id = s.file_id
            WHERE s.name = ?1
            ",
        )?;

        let symbols: Vec<Symbol> = stmt
            .query_map(params![name], |row| {
                let file_path = PathBuf::from(row.get::<_, String>(14)?);
                self.row_to_symbol(row, file_path)
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(symbols)
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT s.id, s.name, s.kind, s.file_id, s.parent_id, s.start_line, s.start_col, s.end_line, s.end_col, s.start_byte, s.end_byte, s.signature, s.description, s.doc_comment, s.visibility, s.is_entry_point, f.path
            FROM symbols s
            JOIN symbols_fts fts ON fts.rowid = s.id
            JOIN files f ON f.id = s.file_id
            WHERE symbols_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            ",
        )?;

        let symbols: Vec<Symbol> = stmt
            .query_map(params![query, limit as i64], |row| {
                let file_path = PathBuf::from(row.get::<_, String>(16)?);
                self.row_to_symbol(row, file_path)
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(symbols)
    }

    fn search_files_fts(&self, query: &str, limit: usize) -> Result<Vec<File>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT f.id, f.path, f.language, f.hash, f.size, f.description
            FROM files f
            JOIN files_fts fts ON fts.rowid = f.id
            WHERE files_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            ",
        )?;

        let files: Vec<File> = stmt
            .query_map(params![query, limit as i64], |row| self.row_to_file(row))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(files)
    }

    fn get_tree(&self) -> Result<Tree> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut file_stmt = conn.prepare(
            "SELECT id, path, language, hash, size, description FROM files ORDER BY path",
        )?;

        let files: Vec<(FileId, PathBuf, Language)> = file_stmt
            .query_map([], |row| {
                let lang_str: String = row.get(2)?;
                Ok((
                    FileId(row.get(0)?),
                    PathBuf::from(row.get::<_, String>(1)?),
                    Language::parse(&lang_str),
                ))
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        let mut nodes = Vec::new();

        for (file_id, path, language) in files {
            let mut symbol_stmt = conn.prepare(
                "SELECT id, name, kind, parent_id FROM symbols WHERE file_id = ?1 ORDER BY start_line",
            )?;

            let symbols: Vec<(i64, String, String, Option<i64>)> = symbol_stmt
                .query_map(params![file_id.0], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .filter_map(std::result::Result::ok)
                .collect();

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

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
        })
    }

    fn update_status(&self, status: &Status) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;

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

        Ok(())
    }

    fn begin_transaction(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;
        conn.execute("BEGIN TRANSACTION", [])?;
        Ok(())
    }

    fn commit_transaction(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;
        conn.execute("COMMIT", [])?;
        Ok(())
    }

    fn rollback_transaction(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Storage(e.to_string()))?;
        conn.execute("ROLLBACK", [])?;
        Ok(())
    }
}
