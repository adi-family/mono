// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

pub mod mmap;
pub mod sqlite;

#[cfg(test)]
mod mmap_tests;
#[cfg(test)]
mod tests;

use crate::error::Result;
use crate::structure::Structure;
use crate::types::{File, FileId, FileInfo, Symbol, SymbolId, SymbolKind, Reference, SymbolUsage, Tree, Status};
use std::path::{Path, PathBuf};

/// One symbol's structural fingerprint, with just enough alongside it to name the symbol in a
/// report.
///
/// Clone detection compares every fingerprint against every other, so it wants the whole set in
/// memory at once and nothing more per row than it will print — loading full [`Symbol`]s (with
/// their signatures and doc comments) would multiply the working set for fields no comparison
/// reads.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StructureRow {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub file_path: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub structure: Structure,
}

impl StructureRow {
    /// Source lines the symbol spans, as a clone report ranks by.
    #[must_use]
    pub fn line_count(&self) -> u32 {
        self.end_line.saturating_sub(self.start_line) + 1
    }
}

pub trait Storage: std::fmt::Debug + Send + Sync {
    // File operations
    fn insert_file(&self, file: &File) -> Result<FileId>;
    fn update_file(&self, file: &File) -> Result<()>;
    fn delete_file(&self, path: &Path) -> Result<()>;
    fn get_file(&self, path: &Path) -> Result<FileInfo>;
    fn get_file_by_id(&self, id: FileId) -> Result<File>;
    fn file_exists(&self, path: &Path) -> Result<bool>;
    fn get_file_hash(&self, path: &Path) -> Result<Option<String>>;

    // Symbol operations
    fn insert_symbol(&self, symbol: &Symbol) -> Result<SymbolId>;
    fn update_symbol(&self, symbol: &Symbol) -> Result<()>;
    fn delete_symbols_for_file(&self, file_id: FileId) -> Result<()>;
    fn get_symbol(&self, id: SymbolId) -> Result<Symbol>;
    fn get_symbols_for_file(&self, file_id: FileId) -> Result<Vec<Symbol>>;
    fn get_all_symbols(&self) -> Result<Vec<Symbol>>;

    // Reference/usage operations
    /// Insert a single reference
    fn insert_reference(&self, reference: &Reference) -> Result<()>;
    /// Insert multiple references in batch (more efficient)
    fn insert_references_batch(&self, references: &[Reference]) -> Result<()>;
    /// Delete every reference with a symbol from this file at either end of it.
    ///
    /// Both ends, because reprocessing a file gives its symbols new ids — `AUTOINCREMENT` never
    /// reuses one — so an edge pointing *into* the old ids can never join a live symbol again.
    /// It stopped being an edge the moment the file was reprocessed; leaving it in the table
    /// only hides that.
    ///
    /// Rebuilding those inbound edges is a separate matter and does not happen here: a run
    /// resolves references parsed *this* run, and an unchanged file parses none, so an edge from
    /// a file that did not change is restored only when that file is itself reprocessed.
    fn delete_references_for_file(&self, file_id: FileId) -> Result<()>;
    /// Get symbols that call/reference this symbol (callers/inbound references)
    fn get_callers(&self, id: SymbolId) -> Result<Vec<Symbol>>;
    /// Get symbols that this symbol calls/references (callees/outbound references)
    fn get_callees(&self, id: SymbolId) -> Result<Vec<Symbol>>;
    /// Get the number of references to a symbol
    fn get_reference_count(&self, id: SymbolId) -> Result<u64>;
    /// Get all references to a symbol with full details
    fn get_references_to(&self, id: SymbolId) -> Result<Vec<Reference>>;
    /// Get all references from a symbol with full details
    fn get_references_from(&self, id: SymbolId) -> Result<Vec<Reference>>;
    /// Find symbols by exact name (for reference resolution)
    fn find_symbols_by_name(&self, name: &str) -> Result<Vec<Symbol>>;
    /// Get full usage statistics for a symbol
    fn get_symbol_usage(&self, id: SymbolId) -> Result<SymbolUsage>;

    // Search operations
    fn search_symbols_fts(&self, query: &str, limit: usize) -> Result<Vec<Symbol>>;
    fn search_files_fts(&self, query: &str, limit: usize) -> Result<Vec<File>>;

    /// Every fingerprinted symbol of at least `min_nodes` named nodes.
    ///
    /// Symbols indexed before migration v3 have no fingerprint and are absent, as are symbols
    /// too small to say anything — see [`crate::structure`] on why the floor matters.
    fn structures(&self, min_nodes: u32) -> Result<Vec<StructureRow>>;

    // Tree operations
    fn get_tree(&self) -> Result<Tree>;

    // Status
    fn get_status(&self) -> Result<Status>;
    fn update_status(&self, status: &Status) -> Result<()>;

    // Transaction support
    fn begin_transaction(&self) -> Result<()>;
    fn commit_transaction(&self) -> Result<()>;
    fn rollback_transaction(&self) -> Result<()>;

    /// Every file the index holds, by id and stored path.
    ///
    /// What an indexing run compares its walk against: a file the index knows and the walk did
    /// not reach is one that left the tree, and nothing else in the pipeline is in a position
    /// to notice.
    fn indexed_files(&self) -> Result<Vec<(FileId, PathBuf)>>;

    /// Remove a file and everything derived from it — its symbols, the references at either end
    /// of them, its reachability verdicts, and its full-text rows.
    ///
    /// Returns the symbol ids that went, because their embeddings live outside SQLite and have
    /// to be dropped from the vector index by the caller.
    fn delete_file_cascade(&self, id: FileId) -> Result<Vec<SymbolId>>;
}
