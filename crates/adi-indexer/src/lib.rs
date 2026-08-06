// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! adi-indexer — the code index behind `adi-mono indexer`.
//!
//! Opening an [`Indexer`] on a project directory gives you three views of its code, all built
//! from one tree-sitter parse per file:
//!
//! * **symbols** — functions, types, methods and their locations, in SQLite with an FTS5 index
//!   over names and doc comments;
//! * **the call graph** — who calls whom, which [`graph`] turns into callers, callees, cycles,
//!   entry points, and (with [`analyzer`]) unreachable code;
//! * **meaning** — a 768-dimension embedding per symbol in a usearch index, so [`Indexer::search`]
//!   answers "where do we retry failed uploads" and not just "where does the word retry appear".
//!
//! Everything a project needs lives under its own `.adi/tree`. What is shared machine-wide is
//! the parse/embedding cache, keyed by content hash — the same file in ten worktrees is parsed
//! and embedded once (see [`GlobalCache`]).
//!
//! ## What moved, and what changed on the way
//!
//! This was three cdylib plugins (`adi.indexer` plus eleven `adi.lang.*`) loaded through a v3
//! plugin ABI at run time. Here the grammars and analyzers are linked in behind `lang-*`
//! features: no plugin directory, no dlopen, no ABI to keep in step — and a binary that either
//! parses a language or admits it cannot.

// The code below arrived from another workspace, one that did not run clippy's pedantic set.
// `cargo clippy --fix` took the mechanical half; what remains are house-style differences
// (undocumented `# Errors` sections, `usize as u32` on tree-sitter offsets that `Location`
// stores as u32 by design, analyzer walkers that run long). They are allowed here rather than
// left to warn, so a real warning in this crate stands out instead of drowning in ~270 of
// these. The list is meant to shrink; nothing in it is load-bearing.
#![allow(
    clippy::missing_errors_doc,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::match_same_arms,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::format_push_string,
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::similar_names,
    clippy::unused_async,
    clippy::unnecessary_wraps,
    clippy::items_after_statements,
    clippy::ref_option,
    clippy::iter_not_returning_iterator,
    clippy::trivially_copy_pass_by_ref,
    clippy::large_enum_variant
)]

pub mod analyzer;
pub mod cache;
pub mod config;
pub mod embed;
pub mod error;
pub mod graph;
pub mod indexer;
pub mod lang;
mod migrations;
pub mod parser;
pub mod paths;
pub mod search;
pub mod storage;
pub mod types;
pub mod watcher;

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod config_tests;

pub use analyzer::{
    AnalysisConfig, AnalysisMode, DeadCodeAnalyzer, DeadCodeFilter, DeadCodeReport,
    EntryPointDetector, ReachabilityAnalyzer, ReportFormat,
};
pub use cache::GlobalCache;
pub use config::Config;
pub use embed::Embedder;
pub use error::{Error, Result};
pub use graph::{
    calculate_metrics, detect_cycles, find_call_path, get_entry_points, get_leaf_nodes,
    get_transitive_callees, get_transitive_callers, get_usage_stats, SymbolMetrics,
};
pub use storage::sqlite::SqliteStorage;
pub use types::*;
pub use watcher::Watcher;

#[cfg(feature = "candle")]
pub use embed::CandleEmbedder;

use crate::parser::Parser;
use crate::search::VectorIndex;
use crate::storage::Storage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

/// An open index over one project directory.
#[derive(Debug)]
pub struct Indexer {
    project_path: PathBuf,
    config: Config,
    storage: Arc<dyn Storage>,
    embedder: Arc<dyn Embedder>,
    parser: Arc<dyn Parser>,
    index: Arc<dyn VectorIndex>,
    cache: Arc<GlobalCache>,
}

impl Indexer {
    /// Open (creating if absent) the index for `project_path`.
    ///
    /// With the `candle` feature this loads the jina embedding model — a few seconds on first
    /// call, and a download on the very first run ever. Without it, indexing and FTS search
    /// work and semantic search reports that this build has no embedder.
    pub async fn open(project_path: &Path) -> Result<Self> {
        #[cfg(feature = "candle")]
        let embedder: Arc<dyn Embedder> = Arc::new(
            embed::CandleEmbedder::new()
                .map_err(|e| Error::Config(format!("Candle embedder init: {e}")))?,
        );
        #[cfg(not(feature = "candle"))]
        let embedder: Arc<dyn Embedder> = Arc::new(embed::NoEmbedder);

        Self::open_with_embedder(project_path, embedder).await
    }

    /// Open the index with an embedder you supply — what the tests use, and the seam for any
    /// other embedding source.
    pub async fn open_with_embedder(
        project_path: &Path,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self> {
        let config = Config::load(project_path)?;
        let adi_dir = project_path.join(".adi");
        let tree_dir = paths::index_dir(project_path);

        std::fs::create_dir_all(&adi_dir)?;
        std::fs::create_dir_all(&tree_dir)?;
        std::fs::create_dir_all(tree_dir.join("embeddings"))?;

        let storage = SqliteStorage::open(&tree_dir.join("index.sqlite"))?;
        let parser = parser::TreeSitterParser::new();
        let index = search::usearch::UsearchIndex::open(&tree_dir.join("embeddings"))?;
        let cache = GlobalCache::open()?;

        Ok(Self {
            project_path: project_path.to_path_buf(),
            config,
            storage: Arc::new(storage),
            embedder,
            parser: Arc::new(parser),
            index: Arc::new(index),
            cache: Arc::new(cache),
        })
    }

    #[must_use]
    pub fn project_path(&self) -> &Path {
        &self.project_path
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The languages this build can parse.
    #[must_use]
    pub fn languages(&self) -> Vec<Language> {
        lang::supported()
    }

    /// The shared storage handle — what the graph and dead-code analyses take.
    #[must_use]
    pub fn storage(&self) -> Arc<dyn Storage> {
        self.storage.clone()
    }

    pub async fn index(&self) -> Result<IndexProgress> {
        indexer::index_project(
            &self.project_path,
            &self.config,
            self.storage.clone(),
            self.embedder.clone(),
            self.parser.clone(),
            self.index.clone(),
            self.cache.clone(),
        )
        .await
    }

    pub async fn reindex(&self, paths: &[PathBuf]) -> Result<()> {
        indexer::reindex_paths(
            &self.project_path,
            paths,
            &self.config,
            self.storage.clone(),
            self.embedder.clone(),
            self.parser.clone(),
            self.index.clone(),
            self.cache.clone(),
        )
        .await
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        search::search(
            query,
            limit,
            self.storage.clone(),
            self.embedder.clone(),
            self.index.clone(),
        )
        .await
    }

    pub async fn search_symbols(&self, query: &str, limit: usize) -> Result<Vec<Symbol>> {
        search::search_symbols(query, limit, self.storage.clone()).await
    }

    pub async fn search_files(&self, query: &str, limit: usize) -> Result<Vec<File>> {
        search::search_files(query, limit, self.storage.clone()).await
    }

    pub fn get_file(&self, path: &Path) -> Result<FileInfo> {
        self.storage.get_file(path)
    }

    pub fn get_symbol(&self, id: SymbolId) -> Result<Symbol> {
        self.storage.get_symbol(id)
    }

    pub fn get_callers(&self, id: SymbolId) -> Result<Vec<Symbol>> {
        self.storage.get_callers(id)
    }

    pub fn get_callees(&self, id: SymbolId) -> Result<Vec<Symbol>> {
        self.storage.get_callees(id)
    }

    pub fn get_reference_count(&self, id: SymbolId) -> Result<u64> {
        self.storage.get_reference_count(id)
    }

    pub fn get_symbol_usage(&self, id: SymbolId) -> Result<SymbolUsage> {
        self.storage.get_symbol_usage(id)
    }

    pub fn find_symbols_by_name(&self, name: &str) -> Result<Vec<Symbol>> {
        self.storage.find_symbols_by_name(name)
    }

    pub fn get_references_to(&self, id: SymbolId) -> Result<Vec<Reference>> {
        self.storage.get_references_to(id)
    }

    pub fn get_references_from(&self, id: SymbolId) -> Result<Vec<Reference>> {
        self.storage.get_references_from(id)
    }

    pub fn get_tree(&self) -> Result<Tree> {
        self.storage.get_tree()
    }

    pub fn status(&self) -> Result<Status> {
        self.storage.get_status()
    }

    pub fn start_watching(&self) -> Result<mpsc::UnboundedReceiver<Vec<PathBuf>>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let watcher = Watcher::new(self.project_path.clone(), Arc::new(self.config.clone()), tx);
        watcher.start()?;
        Ok(rx)
    }
}
