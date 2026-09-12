//! `openai` — any OpenAI-compatible `/v1/embeddings` endpoint, one batched request per call.
//!
//! No prior calibration to preserve here, unlike `ollama`: this runtime does not exist anywhere
//! in the tree today. It takes the more efficient shape — every text in one `input` array — and
//! reorders the answer by its own `index` rather than trusting response order, since the API
//! contract promises the field but not the ordering.

use std::time::Duration;

use adi_indexer::embed::{EmbedError, Embedder};
use serde_json::json;

/// Long enough for a large batch against a cold-started endpoint; short enough that a genuinely
/// unreachable host fails a call rather than hanging it.
const TIMEOUT: Duration = Duration::from_secs(120);

/// An embedder backed by an OpenAI-compatible `/v1/embeddings` endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiEmbedder {
    base_url: String,
    model: String,
    dimensions: u32,
    /// The environment variable the key is read from — a name, not a secret, exactly as
    /// `LlmBackendManifest::api_key_env` carries one. Read fresh on every call rather than once
    /// at construction, so a key rotated by editing the environment takes effect without a
    /// restart.
    api_key_env: Option<String>,
}

impl OpenAiEmbedder {
    /// Point it at a specific endpoint, model, and width — the whole of this runtime's
    /// configuration, read off the backend manifest.
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        dimensions: u32,
        api_key_env: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            dimensions,
            api_key_env,
        }
    }

    fn api_key(&self) -> Result<String, EmbedError> {
        let var = self.api_key_env.as_deref().ok_or_else(|| {
            EmbedError::Config("this backend names no `api_key_env` to read a key from".into())
        })?;
        std::env::var(var).map_err(|_| {
            EmbedError::Config(format!("`{var}` is not set — this backend has no key to send"))
        })
    }
}

impl Embedder for OpenAiEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        super::install_tls_provider();
        let key = self.api_key()?;
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| EmbedError::Embedding(format!("building the http client: {e}")))?;
        let url = format!("{}/v1/embeddings", self.base_url.trim_end_matches('/'));
        let response = client
            .post(&url)
            .bearer_auth(key)
            .json(&json!({"model": self.model, "input": texts}))
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
        let data = answer["data"].as_array().ok_or_else(|| {
            EmbedError::Embedding(format!("{url}: no `data` array in the answer"))
        })?;
        let mut rows: Vec<(usize, Vec<f32>)> = data
            .iter()
            .map(|row| {
                let index = row["index"].as_u64().unwrap_or(0) as usize;
                let vector = row["embedding"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(serde_json::Value::as_f64)
                            .map(|x| x as f32)
                            .collect()
                    })
                    .unwrap_or_default();
                (index, vector)
            })
            .collect();
        rows.sort_by_key(|(index, _)| *index);
        let vectors: Vec<Vec<f32>> = rows.into_iter().map(|(_, v)| v).collect();
        if vectors.len() != texts.len() || vectors.iter().any(Vec::is_empty) {
            return Err(EmbedError::Embedding(format!(
                "{url}: expected {} embeddings, the answer had {}",
                texts.len(),
                vectors.len()
            )));
        }
        Ok(vectors)
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
    fn a_backend_with_no_api_key_env_refuses_before_it_ever_sends_a_request() {
        let embedder = OpenAiEmbedder::new("https://api.openai.test", "text-embed-3", 1536, None);
        let err = embedder.embed(&["hello"]).expect_err("refused");
        assert!(err.to_string().contains("api_key_env"), "{err}");
    }

    #[test]
    fn an_unset_variable_is_reported_by_name() {
        let embedder = OpenAiEmbedder::new(
            "https://api.openai.test",
            "text-embed-3",
            1536,
            Some("ADI_EMBEDDINGS_TEST_UNSET_KEY".into()),
        );
        let err = embedder.embed(&["hello"]).expect_err("refused");
        assert!(
            err.to_string().contains("ADI_EMBEDDINGS_TEST_UNSET_KEY"),
            "{err}"
        );
    }
}
