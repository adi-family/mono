//! The index schema, and the small runner that applies it.
//!
//! Upstream this leaned on `lib-migrations`, a workspace crate with a backend trait, rollback,
//! and migrate-to-version. The indexer only ever migrated forward against SQLite, so what
//! moved is that one path.

mod runner;

pub use runner::{run, SqlMigration};

/// Every migration, in order. The runner refuses a set whose versions are not 1..=n.
pub fn migrations() -> Vec<SqlMigration> {
    vec![migration_v1(), migration_v2()]
}

/// V1: Initial schema - files, symbols, `symbol_refs`, status, FTS
fn migration_v1() -> SqlMigration {
    SqlMigration::new(
        1,
        "initial_schema",
        r"
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            language TEXT NOT NULL,
            hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            description TEXT
        );

        CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            parent_id INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
            start_line INTEGER NOT NULL,
            start_col INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            end_col INTEGER NOT NULL,
            start_byte INTEGER NOT NULL,
            end_byte INTEGER NOT NULL,
            signature TEXT,
            description TEXT,
            doc_comment TEXT,
            visibility TEXT NOT NULL DEFAULT 'unknown',
            is_entry_point INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS symbol_refs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            from_symbol_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
            to_symbol_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            start_col INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            end_col INTEGER NOT NULL,
            start_byte INTEGER NOT NULL,
            end_byte INTEGER NOT NULL,
            UNIQUE(from_symbol_id, to_symbol_id, kind, start_line, start_col)
        );

        CREATE INDEX IF NOT EXISTS idx_refs_from ON symbol_refs(from_symbol_id);
        CREATE INDEX IF NOT EXISTS idx_refs_to ON symbol_refs(to_symbol_id);

        CREATE TABLE IF NOT EXISTS status (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);

        -- FTS for full-text search on symbols
        CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
            name,
            description,
            doc_comment,
            content='symbols',
            content_rowid='id'
        );

        -- FTS for full-text search on files
        CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
            path,
            description,
            content='files',
            content_rowid='id'
        );

        -- Triggers to keep FTS in sync
        CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO symbols_fts(rowid, name, description, doc_comment)
            VALUES (new.id, new.name, new.description, new.doc_comment);
        END;

        CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, description, doc_comment)
            VALUES ('delete', old.id, old.name, old.description, old.doc_comment);
        END;

        CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, description, doc_comment)
            VALUES ('delete', old.id, old.name, old.description, old.doc_comment);
            INSERT INTO symbols_fts(rowid, name, description, doc_comment)
            VALUES (new.id, new.name, new.description, new.doc_comment);
        END;

        CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files BEGIN
            INSERT INTO files_fts(rowid, path, description)
            VALUES (new.id, new.path, new.description);
        END;

        CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files BEGIN
            INSERT INTO files_fts(files_fts, rowid, path, description)
            VALUES ('delete', old.id, old.path, old.description);
        END;

        CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE ON files BEGIN
            INSERT INTO files_fts(files_fts, rowid, path, description)
            VALUES ('delete', old.id, old.path, old.description);
            INSERT INTO files_fts(rowid, path, description)
            VALUES (new.id, new.path, new.description);
        END;
        ",
    )
}

/// V2: Add visibility, `is_entry_point`, and `reachability_cache`
fn migration_v2() -> SqlMigration {
    SqlMigration::new(
        2,
        "add_visibility_and_reachability",
        r"
        -- Note: visibility and is_entry_point columns already exist in v1 schema
        -- This migration adds the indices and reachability cache table

        CREATE INDEX IF NOT EXISTS idx_symbols_visibility ON symbols(visibility);
        CREATE INDEX IF NOT EXISTS idx_symbols_entry_point ON symbols(is_entry_point);

        CREATE TABLE IF NOT EXISTS reachability_cache (
            symbol_id INTEGER PRIMARY KEY REFERENCES symbols(id) ON DELETE CASCADE,
            is_reachable INTEGER NOT NULL,
            last_analyzed INTEGER NOT NULL
        );
        ",
    )
}
