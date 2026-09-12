//! Turning notes into vectors — the indexer's embedder, borrowed whole.
//!
//! [`Embedder`] *is* [`adi_indexer::embed::Embedder`], not a parallel trait that happens to look
//! like it: one trait, so a store can be handed any model the indexer can load and the two never
//! drift apart. A note and a symbol are embedded by the same model here, so they are comparable.
//!
//! **That is a property of this store, not of the workspace.** `adi-facts` uses the same trait
//! and a *different* model — [`TextEmbedder`](adi_indexer::embed::TextEmbedder), prose weights
//! rather than code weights — because a fact is a sentence somebody said and the embedder is the
//! largest single effect on how well such sentences rank against each other. Facts and notes
//! therefore sit in **different vector spaces** and must never be compared by cosine. What keeps
//! that honest is not a convention: every stored vector records the
//! [model](adi_indexer::embed::Embedder::model_name) that made it, and a vector whose model no
//! longer matches is treated as absent.
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

/// The embedder a store uses: whatever was injected, else a lazily built default.
///
/// `adi-facts` holds one of these too, over a different model, which is why it is public rather
/// than private to this crate — the laziness and the cached failure are worth exactly as much
/// there, and a second copy of them would be a second thing to get wrong.
///
/// The failure is cached too. A machine with no network is going to fail on the second note as
/// surely as the first, and retrying a multi-second model load per note would turn one clear
/// error into a store that merely feels broken.
#[derive(Clone)]
pub struct EmbedderSlot {
    injected: Option<Arc<dyn Embedder>>,
    build: Arc<dyn Fn() -> Result<Arc<dyn Embedder>> + Send + Sync>,
    lazy: Arc<OnceLock<std::result::Result<Arc<dyn Embedder>, String>>>,
}

impl Default for EmbedderSlot {
    /// A slot that will load [`default_embedder`] when something first needs a vector.
    fn default() -> Self {
        Self::lazily(default_embedder)
    }
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
    #[must_use]
    pub fn injected(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            injected: Some(embedder),
            build: Arc::new(default_embedder),
            lazy: Arc::default(),
        }
    }

    /// A slot that builds its embedder with `build` on first use — how `adi-facts` asks for the
    /// prose model instead of the code one without reimplementing any of the laziness.
    #[must_use]
    pub fn lazily(build: impl Fn() -> Result<Arc<dyn Embedder>> + Send + Sync + 'static) -> Self {
        Self {
            injected: None,
            build: Arc::new(build),
            lazy: Arc::default(),
        }
    }

    /// The embedder, building it on first use.
    ///
    /// # Errors
    /// [`Error::Embed`] when the model cannot be built — and the same error every time after,
    /// from cache.
    pub fn get(&self) -> Result<Arc<dyn Embedder>> {
        if let Some(embedder) = &self.injected {
            return Ok(embedder.clone());
        }
        self.lazy
            .get_or_init(|| (self.build)().map_err(|e| e.to_string()))
            .clone()
            .map_err(Error::Embed)
    }

    /// The model name without paying to load a model that isn't loaded yet.
    ///
    /// Used where the answer only labels a report — `status`, a listing — so a build that has
    /// never embedded anything doesn't download a model to print a column.
    #[must_use]
    pub fn model_name_if_known(&self) -> Option<String> {
        match (&self.injected, self.lazy.get()) {
            (Some(e), _) => Some(e.model_name().to_string()),
            (None, Some(Ok(e))) => Some(e.model_name().to_string()),
            _ => None,
        }
    }
}

/// A deterministic bag-of-words embedder: no model, no download, no network.
///
/// Moved down into `adi-embeddings` — the registry that must be able to list every embedding
/// provider in one place, this one included — and re-exported here so every existing caller in
/// this crate (and its dependents) keeps working unchanged. See `docs/embedding-backends.md`.
pub use adi_embeddings::HashEmbedder;
/// The width of a [`HashEmbedder`] vector.
pub use adi_embeddings::runtimes::hash::HASH_DIMENSIONS;

#[cfg(test)]
mod tests {
    use super::*;

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
