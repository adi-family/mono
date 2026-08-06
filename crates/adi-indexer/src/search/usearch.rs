// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::{Error, Result};
use crate::search::VectorIndex;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

pub struct UsearchIndex {
    index: Mutex<Index>,
    path: PathBuf,
    dimensions: usize,
}

/// `usearch::Index` is not `Debug`; report what identifies this one instead of its contents.
impl std::fmt::Debug for UsearchIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsearchIndex")
            .field("path", &self.path)
            .field("dimensions", &self.dimensions)
            .finish_non_exhaustive()
    }
}

impl UsearchIndex {
    pub fn open(embeddings_dir: &Path) -> Result<Self> {
        let index_path = embeddings_dir.join("symbols.idx");

        // Default to 768 dimensions (jina-embeddings-v2-base-code)
        let dimensions = 768;

        let options = IndexOptions {
            dimensions,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            connectivity: 16,      // M parameter
            expansion_add: 200,    // ef_construction
            expansion_search: 100, // ef_search
            multi: false,
        };

        let index = Index::new(&options)
            .map_err(|e| Error::Index(format!("Failed to create index: {e}")))?;

        // Try to load existing index or reserve capacity
        if index_path.exists() {
            index
                .load(index_path.to_str().unwrap_or(""))
                .map_err(|e| Error::Index(format!("Failed to load index: {e}")))?;
        } else {
            // Reserve initial capacity for new index
            index
                .reserve(10000)
                .map_err(|e| Error::Index(format!("Failed to reserve index capacity: {e}")))?;
        }

        Ok(Self {
            index: Mutex::new(index),
            path: index_path,
            dimensions,
        })
    }

    pub fn with_config(
        embeddings_dir: &Path,
        dimensions: usize,
        m: usize,
        ef_construction: usize,
        ef_search: usize,
    ) -> Result<Self> {
        let index_path = embeddings_dir.join("symbols.idx");

        let options = IndexOptions {
            dimensions,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            connectivity: m,
            expansion_add: ef_construction,
            expansion_search: ef_search,
            multi: false,
        };

        let index = Index::new(&options)
            .map_err(|e| Error::Index(format!("Failed to create index: {e}")))?;

        if index_path.exists() {
            index
                .load(index_path.to_str().unwrap_or(""))
                .map_err(|e| Error::Index(format!("Failed to load index: {e}")))?;
        } else {
            // Reserve initial capacity for new index
            index
                .reserve(10000)
                .map_err(|e| Error::Index(format!("Failed to reserve index capacity: {e}")))?;
        }

        Ok(Self {
            index: Mutex::new(index),
            path: index_path,
            dimensions,
        })
    }
}

impl VectorIndex for UsearchIndex {
    fn add(&self, id: i64, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dimensions {
            return Err(Error::Index(format!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                vector.len()
            )));
        }

        let index = self.index.lock().map_err(|e| Error::Index(e.to_string()))?;

        index
            .add(id as u64, vector)
            .map_err(|e| Error::Index(format!("Failed to add vector: {e}")))?;

        Ok(())
    }

    fn remove(&self, id: i64) -> Result<()> {
        let index = self.index.lock().map_err(|e| Error::Index(e.to_string()))?;

        index
            .remove(id as u64)
            .map_err(|e| Error::Index(format!("Failed to remove vector: {e}")))?;

        Ok(())
    }

    fn search(&self, query: &[f32], limit: usize) -> Result<Vec<(i64, f32)>> {
        if query.len() != self.dimensions {
            return Err(Error::Index(format!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimensions,
                query.len()
            )));
        }

        let index = self.index.lock().map_err(|e| Error::Index(e.to_string()))?;

        let results = index
            .search(query, limit)
            .map_err(|e| Error::Index(format!("Search failed: {e}")))?;

        Ok(results
            .keys
            .into_iter()
            .zip(results.distances)
            .map(|(k, d)| (k as i64, 1.0 - d)) // Convert distance to similarity
            .collect())
    }

    fn get_vector(&self, id: i64) -> Result<Option<Vec<f32>>> {
        let index = self.index.lock().map_err(|e| Error::Index(e.to_string()))?;

        let mut vector = vec![0f32; self.dimensions];
        let found = index
            .get(id as u64, &mut vector[..])
            .map_err(|e| Error::Index(format!("Failed to read vector: {e}")))?;

        // `get` reports how many vectors it filled in; zero means the key is not in the index.
        Ok((found > 0).then_some(vector))
    }

    fn save(&self) -> Result<()> {
        let index = self.index.lock().map_err(|e| Error::Index(e.to_string()))?;

        index
            .save(self.path.to_str().unwrap_or(""))
            .map_err(|e| Error::Index(format!("Failed to save index: {e}")))?;

        Ok(())
    }

    fn count(&self) -> usize {
        self.index.lock().map(|idx| idx.size()).unwrap_or(0)
    }
}
