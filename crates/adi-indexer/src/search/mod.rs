// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

pub mod usearch;

#[cfg(test)]
mod tests;

use crate::error::Result;
use crate::storage::Storage;
use crate::types::{SearchResult, SymbolId, Symbol, File};
use crate::embed::Embedder;
use std::sync::Arc;

pub trait VectorIndex: std::fmt::Debug + Send + Sync {
    fn add(&self, id: i64, vector: &[f32]) -> Result<()>;
    fn remove(&self, id: i64) -> Result<()>;
    fn search(&self, query: &[f32], limit: usize) -> Result<Vec<(i64, f32)>>;
    fn save(&self) -> Result<()>;
    fn count(&self) -> usize;
}

pub async fn search(
    query: &str,
    limit: usize,
    storage: Arc<dyn Storage>,
    embedder: Arc<dyn Embedder>,
    index: Arc<dyn VectorIndex>,
) -> Result<Vec<SearchResult>> {
    // Generate query embedding
    let embeddings = embedder.embed(&[query])?;
    let query_vec = embeddings.into_iter().next().unwrap_or_default();

    if query_vec.is_empty() {
        return Ok(vec![]);
    }

    // Search vector index directly (handles typos, natural language, etc.)
    let vector_results = index.search(&query_vec, limit)?;

    // Map vector results to symbols
    let mut results = Vec::new();
    for (id, score) in vector_results {
        if let Ok(symbol) = storage.get_symbol(SymbolId(id)) {
            results.push(SearchResult {
                symbol,
                score,
                context: None,
            });
        }
    }

    Ok(results)
}

pub async fn search_symbols(
    query: &str,
    limit: usize,
    storage: Arc<dyn Storage>,
) -> Result<Vec<Symbol>> {
    storage.search_symbols_fts(query, limit)
}

pub async fn search_files(
    query: &str,
    limit: usize,
    storage: Arc<dyn Storage>,
) -> Result<Vec<File>> {
    storage.search_files_fts(query, limit)
}
