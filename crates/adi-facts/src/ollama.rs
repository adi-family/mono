//! One HTTP client for the local model server, shared by the embedder and the classifier.
//!
//! Both halves of the model work in this crate talk to the same ollama on the same host — the
//! embedder to `/api/embeddings`, the classifier to `/api/generate` — so they share a client
//! rather than each opening their own. `ADI_FACTS_OLLAMA` moves both at once, which is what a
//! person changing hosts means.

use std::time::Duration;

use serde_json::Value;

/// Where ollama listens when nothing says otherwise.
pub const DEFAULT_HOST: &str = "http://127.0.0.1:11434";

/// The environment variable that moves it.
pub const HOST_VAR: &str = "ADI_FACTS_OLLAMA";

/// What went wrong reaching the model server.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct OllamaError(pub String);

/// A blocking client for one ollama host.
#[derive(Debug, Clone)]
pub struct Ollama {
    host: String,
    timeout: Duration,
}

impl Default for Ollama {
    fn default() -> Self {
        Self::new()
    }
}

impl Ollama {
    /// The host named by `ADI_FACTS_OLLAMA`, else [`DEFAULT_HOST`].
    #[must_use]
    pub fn new() -> Self {
        Self::at(env_or(HOST_VAR, DEFAULT_HOST))
    }

    /// A client for a specific host.
    #[must_use]
    pub fn at(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            // The prototype's 900s. A cold model load plus a batch of 60 pairs is minutes, not
            // seconds, and a timeout that fires mid-sweep costs the whole batch.
            timeout: Duration::from_secs(900),
        }
    }

    /// The host this talks to, for an error message that has to say where it tried.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// POST a JSON body to one of ollama's endpoints and read the answer back as JSON.
    ///
    /// # Errors
    /// [`OllamaError`] when the client cannot be built, the host cannot be reached, the status is
    /// not a success, or the body is not JSON.
    pub fn post(&self, path: &str, body: &Value) -> Result<Value, OllamaError> {
        // reqwest 0.13 is taken workspace-wide with `rustls-no-provider`, so nobody installs a
        // crypto provider for us and `Client::build` fails until somebody does. Every client site
        // in this tree opens with this line; see the note in the root Cargo.toml. It is
        // idempotent — a second install returns `Err`, which is why the result is dropped.
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let client = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| OllamaError(format!("building the http client: {e}")))?;
        let url = format!("{}/api/{path}", self.host.trim_end_matches('/'));
        let response = client
            .post(&url)
            .json(body)
            .send()
            .map_err(|e| OllamaError(format!("{url}: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            return Err(OllamaError(format!("{url}: {status}: {}", text.trim())));
        }
        response
            .json()
            .map_err(|e| OllamaError(format!("{url}: reading the answer: {e}")))
    }
}

/// An environment variable, treating blank as unset.
pub(crate) fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_variable_is_not_a_host() {
        // An exported-but-empty `ADI_FACTS_OLLAMA` must fall through to the default rather than
        // producing `/api/embeddings` with no host in front of it.
        assert_eq!(env_or("ADI_FACTS_NOT_SET_ANYWHERE", DEFAULT_HOST), DEFAULT_HOST);
        assert_eq!(Ollama::at("http://box:11434/").host(), "http://box:11434/");
    }
}
