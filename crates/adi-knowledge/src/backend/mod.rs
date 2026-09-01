//! Pluggable storage for a knowledge base.
//!
//! A base says which **provider** holds it; the provider opens a [`Backend`] bound to that one
//! base. Everything above this line — scoping, access, chunking, embedding, staleness — is the
//! same whatever the provider is, so a new one (a hosted vector database, a shared Postgres, a
//! read-only mirror of somebody else's corpus) is a single trait away and needs no change here.
//!
//! Two ship in the box:
//!
//! * [`sqlite`] — a SQLite file per base, with FTS5 for text and stored f32 vectors for meaning.
//!   The default, and the one a base gets when nothing says otherwise.
//! * [`memory`] — the same contract, in a `HashMap`, losing everything on drop. What the tests
//!   run against, and what an ephemeral base would use.
//!
//! Register your own with [`Providers::register`].

pub mod memory;
pub mod sqlite;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::note::{EmbeddingState, Knowledge};
use crate::scope::BaseId;

/// The built-in provider names.
pub const SQLITE: &str = "sqlite";
/// The in-memory provider — see the [module docs](self).
pub const MEMORY: &str = "memory";

/// What a base's storage has to be able to do.
///
/// One instance is bound to one base, so nothing here takes a [`BaseId`]: isolation is settled
/// before a backend is ever opened, which means an implementation cannot leak across bases by
/// forgetting to filter.
///
/// Implementations must be safe to call from several threads and several processes — the store
/// is shared by agents, tools, and the control panel at once.
pub trait Backend: std::fmt::Debug + Send + Sync {
    /// The provider that opened this backend.
    fn provider(&self) -> &str;

    /// Insert or replace a note, **exactly as given, embedding state included**.
    ///
    /// One rule falls out of that and implementations must honour it: a note arriving with an
    /// empty [`EmbeddingState`] has its stored vectors dropped. Clearing the state is how the
    /// store says "these no longer describe this text", so there is no way to end up with a row
    /// that claims vectors it does not have — and equally, an edit that leaves the embedded text
    /// alone (a new `source`, say) keeps its vectors instead of paying to make them again.
    fn put(&self, note: &Knowledge) -> Result<()>;

    /// One note by id, or `None`.
    fn get(&self, id: &str) -> Result<Option<Knowledge>>;

    /// Notes matching `query`, newest first.
    fn list(&self, query: &Query) -> Result<Vec<Knowledge>>;

    /// Delete a note and everything derived from it. `false` if it wasn't there.
    fn delete(&self, id: &str) -> Result<bool>;

    /// How many notes the base holds.
    fn count(&self) -> Result<usize>;

    /// Replace a note's vectors and record what they were made from. An empty `vectors` clears
    /// them, which is how a note that can no longer be embedded is marked honestly rather than
    /// left claiming vectors it doesn't have.
    fn set_vectors(&self, id: &str, state: &EmbeddingState, vectors: &[Vec<f32>]) -> Result<()>;

    /// The closest chunks to `query`, best first, at most `limit` of them.
    fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<ChunkHit>>;

    /// Full-text search over titles, bodies, and tags, best first.
    fn search_text(&self, query: &str, limit: usize) -> Result<Vec<ChunkHit>>;

    /// Drop every note in the base — what deleting the base runs first.
    fn clear(&self) -> Result<()>;
}

/// What a [`Backend::list`] is asked for.
///
/// Deliberately narrower than the public [`Filter`](crate::Filter): "only the stale ones" is a
/// question about the *current* embedding model, which the store knows and a backend does not,
/// so the store answers it rather than making every provider re-derive it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Keep only notes carrying every one of these tags (already normalized).
    pub tags: Vec<String>,
    /// Cap the result.
    pub limit: Option<usize>,
}

/// One matching chunk: which note, which chunk of it, and how well it matched.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHit {
    /// The note's id.
    pub id: String,
    /// Which chunk matched — 0 for a note that fits in one.
    pub chunk: u32,
    /// Higher is better, in `0.0..=1.0`.
    pub score: f32,
}

/// Everything a provider needs to open one base.
#[derive(Debug, Clone)]
pub struct BaseContext {
    /// Which base this is.
    pub id: BaseId,
    /// The base's own directory in the store, already created.
    pub dir: PathBuf,
    /// Free-form provider settings from the base manifest (a connection string, a collection
    /// name, an api-key secret reference — whatever a given provider needs).
    pub settings: BTreeMap<String, String>,
}

impl BaseContext {
    /// A setting by key.
    #[must_use]
    pub fn setting(&self, key: &str) -> Option<&str> {
        self.settings.get(key).map(String::as_str)
    }
}

/// Opens backends of one kind. This is the extension point.
pub trait Provider: std::fmt::Debug + Send + Sync {
    /// The name a base manifest names this provider by.
    fn name(&self) -> &str;

    /// One line about what it stores knowledge in, for `knowledge providers`.
    fn description(&self) -> &str;

    /// Open (creating if needed) the backend for one base.
    fn open(&self, ctx: &BaseContext) -> Result<Arc<dyn Backend>>;
}

/// The registry of known providers. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Providers {
    map: BTreeMap<String, Arc<dyn Provider>>,
}

impl Default for Providers {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Providers {
    /// The providers that ship in the box: [`SQLITE`] and [`MEMORY`].
    #[must_use]
    pub fn builtin() -> Self {
        let mut map: BTreeMap<String, Arc<dyn Provider>> = BTreeMap::new();
        for provider in [
            Arc::new(sqlite::SqliteProvider) as Arc<dyn Provider>,
            Arc::new(memory::MemoryProvider::default()) as Arc<dyn Provider>,
        ] {
            map.insert(provider.name().to_string(), provider);
        }
        Self { map }
    }

    /// An empty registry — for a build that wants only its own providers.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    /// Add a provider, replacing (and returning) any of the same name.
    pub fn register(&mut self, provider: Arc<dyn Provider>) -> Option<Arc<dyn Provider>> {
        self.map.insert(provider.name().to_string(), provider)
    }

    /// Look one up.
    ///
    /// # Errors
    /// [`Error::NoSuchProvider`] when nothing is registered under `name`.
    pub fn get(&self, name: &str) -> Result<&Arc<dyn Provider>> {
        self.map
            .get(name)
            .ok_or_else(|| Error::NoSuchProvider(name.to_string()))
    }

    /// Every registered provider, by name.
    #[must_use]
    pub fn all(&self) -> Vec<&Arc<dyn Provider>> {
        self.map.values().collect()
    }

    /// Whether a provider is registered under `name`.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }
}

/// Pack f32s into little-endian bytes, for a backend that stores a vector as a blob.
#[must_use]
pub fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Unpack [`encode_vector`]'s bytes. A trailing partial float is ignored rather than guessed at.
#[must_use]
pub fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Cosine similarity, clamped to `0.0..=1.0`.
///
/// Clamped, not merely computed: floating-point error puts an identical pair a hair over 1.0,
/// and negative similarity is not a useful thing to rank by or to show — a result *less* related
/// than unrelated is just unrelated.
#[must_use]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(0.0, 1.0)
}

/// Keep the best chunk per note, then the best `limit` notes.
///
/// A long note splits into many chunks and several of them may match; counting it once, at its
/// best chunk, is what stops one thorough note from filling every slot of a result page.
#[must_use]
pub fn best_per_note(mut hits: Vec<ChunkHit>, limit: usize) -> Vec<ChunkHit> {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert(h.id.clone()));
    hits.truncate(limit);
    hits
}

/// Turn a person's query into an FTS5 MATCH expression that cannot be a syntax error.
///
/// Every token is quoted, so `AND`, `*`, `"`, and `NEAR(` are searched for rather than obeyed,
/// and the tokens are joined with `OR` — a five-word question that demanded all five would
/// usually find nothing, and full-text here is the fallback path, not the precise one.
#[must_use]
pub fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Map SQLite's `bm25()` rank (negative, more-negative is better) onto `0.0..1.0`, monotonically.
///
/// A text score and a cosine score are never compared to each other — the store runs one search
/// or the other — so what this needs is order preservation and a familiar range, not calibration.
#[must_use]
pub fn bm25_score(rank: f64) -> f32 {
    let relevance = (-rank).max(0.0);
    (relevance / (1.0 + relevance)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_survive_the_blob_round_trip() {
        let v = vec![0.5f32, -1.25, 0.0, 3.5e-7];
        assert_eq!(decode_vector(&encode_vector(&v)), v);
        // A truncated blob yields the floats that are whole, not a panic.
        assert_eq!(decode_vector(&[0, 0, 0]).len(), 0);
    }

    #[test]
    fn cosine_is_one_for_a_vector_with_itself_and_zero_for_the_orthogonal() {
        let a = vec![1.0f32, 2.0, 3.0];
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        // Opposed vectors are not "worse than unrelated" — they are unrelated.
        assert_eq!(cosine(&[1.0, 0.0], &[-1.0, 0.0]), 0.0);
        // Mismatched widths and zero vectors are answerable, not a panic.
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn a_note_is_counted_once_at_its_best_chunk() {
        let hits = vec![
            ChunkHit {
                id: "a".into(),
                chunk: 0,
                score: 0.4,
            },
            ChunkHit {
                id: "a".into(),
                chunk: 7,
                score: 0.9,
            },
            ChunkHit {
                id: "b".into(),
                chunk: 0,
                score: 0.6,
            },
        ];
        let best = best_per_note(hits, 10);
        assert_eq!(best.len(), 2);
        assert_eq!((best[0].id.as_str(), best[0].chunk), ("a", 7));
        assert_eq!(best[1].id, "b");
    }

    #[test]
    fn an_fts_query_cannot_be_a_syntax_error() {
        assert_eq!(
            fts_query("restart the panel"),
            "\"restart\" OR \"the\" OR \"panel\""
        );
        // Operators and quotes are searched for, not obeyed.
        assert_eq!(fts_query("NEAR( \"x"), "\"NEAR(\" OR \"x\"");
        assert_eq!(fts_query(""), "");
    }

    #[test]
    fn bm25_keeps_sqlites_order_while_looking_like_a_score() {
        assert!(bm25_score(-9.0) > bm25_score(-1.0));
        assert!((0.0..=1.0).contains(&bm25_score(-3.0)));
        assert_eq!(bm25_score(1.0), 0.0);
    }

    #[test]
    fn the_builtin_registry_holds_both_providers_and_takes_more() {
        let mut providers = Providers::builtin();
        assert!(providers.has(SQLITE) && providers.has(MEMORY));
        assert!(providers.get("nowhere").is_err());
        assert_eq!(providers.all().len(), 2);

        // Registering over a name replaces it, and says what it replaced.
        let replaced = providers.register(Arc::new(memory::MemoryProvider::default()));
        assert!(replaced.is_some());
        assert_eq!(providers.all().len(), 2);
    }
}
