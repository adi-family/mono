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

use adi_indexer::embed::{EmbedError, Embedder};
use serde_json::json;

use crate::ollama::{Ollama, env_or};

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
pub struct OllamaEmbedder {
    ollama: Ollama,
    model: String,
}

impl Default for OllamaEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaEmbedder {
    /// The embedder described by `ADI_FACTS_OLLAMA` and `ADI_FACTS_EMBED`, else the defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ollama: Ollama::new(),
            model: env_or(MODEL_VAR, DEFAULT_MODEL),
        }
    }

    /// Point it at a specific host and model.
    #[must_use]
    pub fn at(host: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            ollama: Ollama::at(host),
            model: model.into(),
        }
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let answer = self
            .ollama
            .post("embeddings", &json!({"model": self.model, "prompt": text}))
            .map_err(|e| EmbedError::Embedding(format!("embedding with {}: {e}", self.model)))?;
        let raw = answer["embedding"].as_array().ok_or_else(|| {
            EmbedError::Embedding(format!(
                "{}: no `embedding` array in the answer — is `{}` pulled? (`ollama pull {}`)",
                self.ollama.host(),
                self.model,
                self.model
            ))
        })?;
        let vector: Vec<f32> = raw
            .iter()
            .filter_map(serde_json::Value::as_f64)
            .map(|v| v as f32)
            .collect();
        if vector.len() != raw.len() || vector.is_empty() {
            return Err(EmbedError::Embedding(format!(
                "{}: the embedding was not a vector of numbers",
                self.model
            )));
        }
        // Normalized here rather than at every comparison, exactly as the prototype does, so a
        // cosine is a dot product and the cached blob is directly comparable to any other.
        let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm <= 0.0 {
            return Err(EmbedError::Embedding(format!(
                "{}: the embedding was all zeros",
                self.model
            )));
        }
        Ok(vector.into_iter().map(|x| x / norm).collect())
    }
}

impl Embedder for OllamaEmbedder {
    /// One request per text.
    ///
    /// `/api/embeddings` takes a single `prompt`, which is what the prototype calls and what the
    /// measurements were taken through. Newer ollama also serves `/api/embed` with an `input`
    /// array; switching would mean the requests are batched differently from the ones the numbers
    /// came from, for a saving that is round trips to localhost.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn dimensions(&self) -> u32 {
        DIMENSIONS
    }

    fn model_name(&self) -> &str {
        &self.model
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
