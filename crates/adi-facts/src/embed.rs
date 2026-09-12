//! Embedding facts: `nomic-embed-text`, served by the same local ollama as the classifier.
//!
//! # Why this model and not the workspace's
//!
//! `adi-indexer` and `adi-knowledge` embed with jina-embeddings-v2-base-code on candle, and this
//! crate deliberately does not. **Every threshold in this design was measured against
//! `nomic-embed-text`** — the recall table, the band structure where `duplicate` sits around 0.82
//! and `controversy` around 0.67 (`RESULTS.md` §8, §9). A different model does
//! not shift those numbers, it invalidates them: the experiment measured the same fourteen
//! related pairs landing inside the top 125 of 528 with this model, the top 166 with
//! `embeddinggemma`, and the top 465 with `mxbai-embed-large`. Paying does not help either —
//! every hosted model tried, `gemini-embedding-001` at 3072 dimensions included, ranked *worse*,
//! because it compresses every pair into a narrow high band and ranking needs spread.
//!
//! So the model is not a detail to be settled by what the workspace already loads. Using the one
//! the calibration came from is the only way those numbers mean anything.
//!
//! # Why over HTTP rather than in-process
//!
//! The classifier is already an ollama client talking to the same host, so the embedder joins it
//! there and the crate carries no model stack at all: no candle, no weights, no download, and no
//! `Embedder` that takes four minutes on first use.
//!
//! [`Embedder`] is still `adi_indexer::embed::Embedder`, the same trait the rest of the
//! workspace implements — so a caller can inject any of them, and a test can inject
//! [`HashEmbedder`](adi_knowledge::HashEmbedder) and never touch a network. What must never
//! happen is a *vector* from one model being compared with a vector from another; that is
//! guarded by storing the model's name beside every cached vector and treating a row from any
//! other model as absent.
//!
//! # Where the HTTP core actually lives now
//!
//! [`OllamaEmbedder`] here is a thin wrapper around `adi_embeddings::OllamaEmbedder`, which moved
//! down into the embedding backend registry — a registry that lists every embedding provider must
//! be able to list this one too (`docs/embedding-backends.md`). That inner type is deliberately
//! **env-agnostic**: it takes a host and a model as plain arguments. Reading `ADI_FACTS_OLLAMA`
//! and `ADI_FACTS_EMBED` stays here, so `OllamaEmbedder::new()`/`::at()` keep working exactly as
//! every existing caller and test expects.

use adi_indexer::embed::{EmbedError, Embedder};

use crate::ollama::env_or;

/// The model every threshold in this design was measured against.
pub const DEFAULT_MODEL: &str = "nomic-embed-text";

/// The environment variable that changes it — and invalidates every measured number when it does.
pub const MODEL_VAR: &str = "ADI_FACTS_EMBED";

/// `nomic-embed-text`'s width.
///
/// Reported so the trait has an answer; nothing depends on it being right. The vector cache
/// validates a stored blob against the width recorded *with that blob*, not against this, so
/// pointing [`MODEL_VAR`] at a model of another width re-embeds rather than misreads.
const DIMENSIONS: u32 = 768;

/// An embedder backed by a local ollama.
#[derive(Debug, Clone)]
pub struct OllamaEmbedder(adi_embeddings::OllamaEmbedder);

impl Default for OllamaEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaEmbedder {
    /// The embedder described by `ADI_FACTS_OLLAMA` and `ADI_FACTS_EMBED`, else the defaults.
    #[must_use]
    pub fn new() -> Self {
        let host = crate::ollama::Ollama::new().host().to_string();
        Self::at(host, env_or(MODEL_VAR, DEFAULT_MODEL))
    }

    /// Point it at a specific host and model.
    #[must_use]
    pub fn at(host: impl Into<String>, model: impl Into<String>) -> Self {
        Self(adi_embeddings::OllamaEmbedder::new(host, model, DIMENSIONS))
    }
}

impl Embedder for OllamaEmbedder {
    /// One request per text — see `adi_embeddings::runtimes::ollama`, which carries this
    /// crate's own calibration note in full.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.0.embed(texts)
    }

    fn dimensions(&self) -> u32 {
        self.0.dimensions()
    }

    fn model_name(&self) -> &str {
        self.0.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_name_travels_with_the_vectors_it_makes() {
        // The name is what a cached vector is matched against, so it has to be the *model*, not
        // the crate's idea of a default: swapping `ADI_FACTS_EMBED` must invalidate the cache
        // rather than silently reuse vectors from another space.
        let embedder = OllamaEmbedder::at("http://box:11434", "mxbai-embed-large");
        assert_eq!(embedder.model_name(), "mxbai-embed-large");
        assert_eq!(OllamaEmbedder::new().model_name(), DEFAULT_MODEL);
    }
}
