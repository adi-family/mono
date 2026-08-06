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
use crate::types::{File, FileId, FileInfo, Symbol, SymbolId, Reference, SymbolUsage, Tree, Status};
use std::path::Path;

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
    /// Delete all references originating from symbols in a file
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

    // Tree operations
    fn get_tree(&self) -> Result<Tree>;

    // Status
    fn get_status(&self) -> Result<Status>;
    fn update_status(&self, status: &Status) -> Result<()>;

    // Transaction support
    fn begin_transaction(&self) -> Result<()>;
    fn commit_transaction(&self) -> Result<()>;
    fn rollback_transaction(&self) -> Result<()>;
}
