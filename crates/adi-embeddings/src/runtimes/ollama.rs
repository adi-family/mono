//! `ollama` — a local model server's `/api/embeddings`, one request per text.
//!
//! The HTTP core moved down from `adi-facts` (`adi_facts::embed::OllamaEmbedder` wraps this type
//! so every existing caller keeps working unchanged), and it is deliberately **env-agnostic**:
//! `adi-facts`'s version read `ADI_FACTS_OLLAMA`/`ADI_FACTS_EMBED` at construction, but a
//! registry backend's host and model are dials on the manifest, not ambient configuration — those
//! two environment variables are honoured once, at seed time, by [`crate::seed`], and never read
//! again. See "Migration" in `docs/embedding-backends.md`.

use std::time::Duration;

use adi_indexer::embed::{EmbedError, Embedder};
use serde_json::json;

/// The prototype's 900s. A cold model load plus a batch of many texts is minutes, not seconds,
/// and a timeout that fires mid-sweep costs the whole batch. Matches `adi_facts::ollama::Ollama`.
const TIMEOUT: Duration = Duration::from_secs(900);

/// An embedder backed by a local ollama.
#[derive(Debug, Clone)]
pub struct OllamaEmbedder {
    host: String,
    model: String,
    dimensions: u32,
}

impl OllamaEmbedder {
    /// Point it at a specific host and model — the whole of this runtime's configuration, read
    /// off the backend manifest's `base_url`, `model` and `dimensions` fields.
    ///
    /// `dimensions` is carried, not measured: ollama's answer never states a width up front, and
    /// nothing here validates a vector against it (see [`Embedder::dimensions`]) — it exists so
    /// the reported width matches whatever the manifest declares rather than a guess.
    #[must_use]
    pub fn new(host: impl Into<String>, model: impl Into<String>, dimensions: u32) -> Self {
        Self {
            host: host.into(),
            model: model.into(),
            dimensions,
        }
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        super::install_tls_provider();
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| EmbedError::Embedding(format!("building the http client: {e}")))?;
        let url = format!("{}/api/embeddings", self.host.trim_end_matches('/'));
        let response = client
            .post(&url)
            .json(&json!({"model": self.model, "prompt": text}))
            .send()
            .map_err(|e| EmbedError::Embedding(format!("{url}: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            return Err(EmbedError::Embedding(format!(
                "{url}: {status}: {}",
                text.trim()
            )));
        }
        let answer: serde_json::Value = response
            .json()
            .map_err(|e| EmbedError::Embedding(format!("{url}: reading the answer: {e}")))?;
        let raw = answer["embedding"].as_array().ok_or_else(|| {
            EmbedError::Embedding(format!(
                "{}: no `embedding` array in the answer — is `{}` pulled? (`ollama pull {}`)",
                self.host, self.model, self.model
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
        // Normalized here rather than at every comparison, matching `adi-facts`'s prior
        // behaviour, so a cosine is a dot product and the cached blob is directly comparable to
        // any other.
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
    /// `/api/embeddings` takes a single `prompt`, which is what `adi-facts`'s calibration was
    /// measured through. Newer ollama also serves `/api/embed` with an `input` array; switching
    /// would mean the requests are batched differently from the ones the numbers came from, for a
    /// saving that is round trips to localhost.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn dimensions(&self) -> u32 {
        self.dimensions
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
        let embedder = OllamaEmbedder::new("http://box:11434", "mxbai-embed-large", 1024);
        assert_eq!(embedder.model_name(), "mxbai-embed-large");
        assert_eq!(embedder.dimensions(), 1024);
    }
}
