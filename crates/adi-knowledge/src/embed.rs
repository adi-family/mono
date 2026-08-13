//! Turning notes into vectors — the indexer's embedder, borrowed whole.
//!
//! [`Embedder`] *is* [`adi_indexer::embed::Embedder`], not a parallel trait that happens to look
//! like it. That is what "reuse the indexer's embedder" has to mean if a note and a symbol are
//! ever to be compared, or the same model loaded once for both: a second trait would have made
//! the two vector spaces incomparable by construction, however identical the model behind them.
//!
//! Loading is **lazy**. The candle model costs seconds on first use and a download on the very
//! first run ever, and most of what the store does — adding, listing, editing, deleting, reading
//! — needs no vectors at all. So the model is built on the first call that genuinely needs it
//! and kept for the life of the store.

use std::sync::{Arc, OnceLock};

use crate::error::{Error, Result};

pub use adi_indexer::embed::{EmbedError, Embedder};

/// The embedder this build uses when nothing else is injected.
///
/// With the `candle` feature that is jina-embeddings-v2-base-code, exactly as the indexer runs
/// it. Without it, [`HashEmbedder`] — which is honest about being a word-overlap stand-in, and
/// is better than a store that cannot search at all.
///
/// # Errors
/// [`Error::Embed`] when the model cannot be loaded (no network on a first run, a corrupt
/// cache, no memory for the weights).
pub fn default_embedder() -> Result<Arc<dyn Embedder>> {
    // Two cfg'd `let`s rather than two cfg'd blocks: an attribute applies to a *statement*, and a
    // cfg'd block in tail position is not one. Same shape the indexer's `Indexer::open` uses.
    #[cfg(feature = "candle")]
    let embedder: Arc<dyn Embedder> = Arc::new(
        adi_indexer::embed::CandleEmbedder::new()
            .map_err(|e| Error::Embed(format!("loading the embedding model: {e}")))?,
    );
    #[cfg(not(feature = "candle"))]
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder);
    Ok(embedder)
}

/// The embedder a store uses: whatever was injected, else [`default_embedder`], built once.
///
/// The failure is cached too. A machine with no network is going to fail on the second note as
/// surely as the first, and retrying a multi-second model load per note would turn one clear
/// error into a store that merely feels broken.
#[derive(Clone, Default)]
pub(crate) struct EmbedderSlot {
    injected: Option<Arc<dyn Embedder>>,
    lazy: Arc<OnceLock<std::result::Result<Arc<dyn Embedder>, String>>>,
}

impl std::fmt::Debug for EmbedderSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match (&self.injected, self.lazy.get()) {
            (Some(e), _) => format!("injected({})", e.model_name()),
            (None, Some(Ok(e))) => format!("loaded({})", e.model_name()),
            (None, Some(Err(e))) => format!("failed({e})"),
            (None, None) => "not yet loaded".to_string(),
        };
        f.debug_tuple("EmbedderSlot").field(&state).finish()
    }
}

impl EmbedderSlot {
    /// A slot holding a caller-supplied embedder.
    pub(crate) fn injected(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            injected: Some(embedder),
            lazy: Arc::default(),
        }
    }

    /// The embedder, building it on first use.
    pub(crate) fn get(&self) -> Result<Arc<dyn Embedder>> {
        if let Some(embedder) = &self.injected {
            return Ok(embedder.clone());
        }
        self.lazy
            .get_or_init(|| default_embedder().map_err(|e| e.to_string()))
            .clone()
            .map_err(Error::Embed)
    }

    /// The model name without paying to load a model that isn't loaded yet.
    ///
    /// Used where the answer only labels a report — `status`, a listing — so a build that has
    /// never embedded anything doesn't download a model to print a column.
    pub(crate) fn model_name_if_known(&self) -> Option<String> {
        match (&self.injected, self.lazy.get()) {
            (Some(e), _) => Some(e.model_name().to_string()),
            (None, Some(Ok(e))) => Some(e.model_name().to_string()),
            _ => None,
        }
    }
}

/// A deterministic bag-of-words embedder: no model, no download, no network.
///
/// Each token is hashed into one of [`HASH_DIMENSIONS`] buckets and the vector is L2-normalized,
/// so cosine similarity between two texts is essentially their **word overlap**.
///
/// Be clear about what that is and isn't. It finds "restart the panel" from "panel restart", and
/// it will never find it from "bring the control surface back up" — it has no idea the two mean
/// the same thing, which is the entire point of a real embedding model. It is here so that tests
/// can assert ranking without a 300MB download, and so a `--no-default-features` build degrades
/// to something that works rather than to nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashEmbedder;

/// The width of a [`HashEmbedder`] vector. Wide enough that unrelated words rarely collide,
/// narrow enough to be cheap.
pub const HASH_DIMENSIONS: u32 = 256;

impl Embedder for HashEmbedder {
    fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| hash_vector(t)).collect())
    }

    fn dimensions(&self) -> u32 {
        HASH_DIMENSIONS
    }

    fn model_name(&self) -> &str {
        "hash-bow-256"
    }
}

fn hash_vector(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; HASH_DIMENSIONS as usize];
    for token in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        let bucket = fnv1a(&token.to_lowercase()) as usize % v.len();
        v[bucket] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// FNV-1a, 64-bit — a stable hash, unlike `DefaultHasher`, whose output is not promised to be
/// the same across Rust releases. Vectors written today must still match a query tomorrow.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::cosine;

    #[test]
    fn the_same_text_always_embeds_to_the_same_vector() {
        let e = HashEmbedder;
        let a = e.embed(&["restart the panel"]).expect("embed");
        let b = e.embed(&["restart the panel"]).expect("embed");
        assert_eq!(a, b);
        assert_eq!(a[0].len(), HASH_DIMENSIONS as usize);
    }

    #[test]
    fn word_order_and_case_do_not_change_the_vector() {
        let e = HashEmbedder;
        let a = e.embed(&["Restart The Panel"]).expect("embed");
        let b = e.embed(&["panel restart the"]).expect("embed");
        assert!((cosine(&a[0], &b[0]) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn overlapping_text_scores_above_unrelated_text() {
        let e = HashEmbedder;
        let query = &e.embed(&["restart the control panel"]).expect("embed")[0];
        let close = &e.embed(&["how to restart the panel"]).expect("embed")[0];
        let far = &e.embed(&["sourdough hydration ratios"]).expect("embed")[0];
        assert!(cosine(query, close) > cosine(query, far));
        assert_eq!(cosine(query, far), 0.0);
    }

    #[test]
    fn a_batch_comes_back_in_order_and_empty_text_is_a_zero_vector() {
        let e = HashEmbedder;
        let out = e.embed(&["alpha", "", "beta"]).expect("embed");
        assert_eq!(out.len(), 3);
        assert!(out[1].iter().all(|x| *x == 0.0));
        assert_ne!(out[0], out[2]);
    }

    #[test]
    fn an_injected_embedder_is_used_instead_of_loading_a_model() {
        let slot = EmbedderSlot::injected(Arc::new(HashEmbedder));
        assert_eq!(slot.get().expect("get").model_name(), "hash-bow-256");
        assert_eq!(slot.model_name_if_known().as_deref(), Some("hash-bow-256"));
    }

    #[test]
    fn an_unused_slot_never_loads_anything() {
        let slot = EmbedderSlot::default();
        assert_eq!(slot.model_name_if_known(), None);
        assert!(format!("{slot:?}").contains("not yet loaded"));
    }
}
