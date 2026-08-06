// Global content-addressable cache in the indexer's module of the mono store.
//
// Stores parsed symbols and embeddings keyed by SHA256 of file content.
// Shared across all projects and worktrees — same file content is indexed once.

use crate::error::{Error, Result};
use crate::types::ParsedFile;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::debug;

/// What the cache stores, as a number to compare against.
///
/// The key is a hash of the file's *content*, so nothing about a change to this crate reaches
/// it: same file, same key, and an entry written by an older build is handed back as though it
/// were current. That is fine while the entry means the same thing, and silently wrong the
/// moment it does not — a `ParsedSymbol` that gained a field parses back without it, and an
/// embedding built from a different text is a plausible vector for the wrong thing, with no
/// symptom beyond worse answers.
///
/// So: bump this whenever either half changes shape or meaning — and note that "meaning"
/// includes any edit to `indexer::build_embed_text`, because the cache key cannot see it.
/// Skipping the bump does not fail; it returns in seconds with the old vectors and identical
/// scores, which looks exactly like a change that did not help. Reparsing a file costs
/// milliseconds.
///
/// * 1 — symbols carry a structural fingerprint, and embeddings are built from a text that
///   includes the symbol's body.
/// * 2 — the declaration is repeated after the body, so it keeps a meaningful share of the
///   pooled vector.
pub const SCHEMA_VERSION: u32 = 2;

/// Cached parsing + embedding results for a single file content hash.
#[derive(Debug, Serialize, Deserialize)]
pub struct CachedFileData {
    pub parsed: ParsedFile,
    pub embeddings: Vec<Vec<f32>>,
    pub embed_model: String,
    /// See [`SCHEMA_VERSION`]. Entries written before this field existed deserialize to 0,
    /// which is not a valid version and so reads as stale.
    #[serde(default)]
    pub schema_version: u32,
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

        // An entry from an older layout is not partly usable: whatever changed may have changed
        // the parse as easily as the embeddings. Treat it as absent — `put` overwrites it in
        // place, so the stale copy does not linger.
        if cached.schema_version != SCHEMA_VERSION {
            debug!(
                "cache miss (schema {} != {SCHEMA_VERSION}): {hash}",
                cached.schema_version
            );
            return None;
        }

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
                schema_version: SCHEMA_VERSION,
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
