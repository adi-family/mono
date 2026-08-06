// Global content-addressable cache in the indexer's module of the mono store.
//
// Stores parsed symbols and embeddings keyed by SHA256 of file content.
// Shared across all projects and worktrees — same file content is indexed once.

use crate::error::{Error, Result};
use crate::types::ParsedFile;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Cached parsing + embedding results for a single file content hash.
#[derive(Debug, Serialize, Deserialize)]
pub struct CachedFileData {
    pub parsed: ParsedFile,
    pub embeddings: Vec<Vec<f32>>,
    pub embed_model: String,
}

#[derive(Debug)]
pub struct GlobalCache {
    base_dir: PathBuf,
}

impl GlobalCache {
    /// Open the machine-wide cache (`~/.adi/mono/indexer/cache` unless `$ADI_DIR` says else).
    pub fn open() -> Result<Self> {
        let base_dir = crate::paths::cache_dir();
        std::fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Open a cache at a custom path (for testing).
    pub fn open_at(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        Ok(Self {
            base_dir: path.to_path_buf(),
        })
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        let prefix = &hash[..2.min(hash.len())];
        self.base_dir.join(prefix).join(format!("{hash}.json"))
    }

    /// Look up a cached file by content hash.
    /// Returns full cache entry if `embed_model` matches, or just parsed data with empty embeddings.
    pub fn get(&self, hash: &str, embed_model: &str) -> Option<CachedFileData> {
        let path = self.object_path(hash);
        let data = std::fs::read_to_string(&path).ok()?;
        let cached: CachedFileData = serde_json::from_str(&data).ok()?;

        if cached.embed_model == embed_model {
            debug!("cache hit (full): {hash}");
            Some(cached)
        } else {
            // Model changed — return parsed data, discard stale embeddings
            debug!("cache hit (parsed only, model mismatch): {hash}");
            Some(CachedFileData {
                parsed: cached.parsed,
                embeddings: Vec::new(),
                embed_model: String::new(),
            })
        }
    }

    /// Store parsed + embedding data for a file content hash.
    pub fn put(&self, hash: &str, data: &CachedFileData) -> Result<()> {
        let path = self.object_path(hash);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(data)
            .map_err(|e| Error::Storage(format!("cache serialization: {e}")))?;
        std::fs::write(&path, json)?;
        debug!("cache put: {hash}");
        Ok(())
    }

    /// Check if a hash exists in the cache (without reading).
    #[must_use]
    pub fn contains(&self, hash: &str) -> bool {
        self.object_path(hash).exists()
    }
}
