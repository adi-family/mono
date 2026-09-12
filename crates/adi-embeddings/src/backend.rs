//! The backend definition, its on-disk store, and turning one into a working [`Embedder`].
//!
//! One file per backend, `embeddings/backends/<id>.toml`, the id taken from the filename exactly
//! as `llm/backends/<id>.toml` takes its (there is no `id` field to drift from what the file is
//! called):
//!
//! ```toml
//! label      = "Local ollama — nomic-embed-text"
//! runtime    = "ollama"
//! model      = "nomic-embed-text"
//! dimensions = 768
//! base_url   = "http://127.0.0.1:11434"
//!
//! fallbacks = ["ollama-hosted"]
//! ```
//!
//! See `docs/embedding-backends.md` for the full design and why failover here is deliberately
//! narrower than the LLM design's.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use adi_config::{Config, Module};
use adi_indexer::embed::{EmbedError, Embedder};

use crate::error::{Error, Result};
use crate::runtimes::{hash::HashEmbedder, ollama::OllamaEmbedder, openai::OpenAiEmbedder};
use crate::{BACKENDS_DIR, EMBEDDINGS_MODULE};

/// Which of the four runtimes builds this backend's [`Embedder`].
///
/// Named `runtime`, not `provider`: the survey behind this design (`docs/embedding-backends-
/// survey.md` §7) found "provider" already meaning three different things in this codebase
/// before this field would have added a fourth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// In-process, `adi_indexer::embed::CandleEmbedder` — fixed to
    /// jina-embeddings-v2-base-code. Exists only in a binary built with the `candle` cargo
    /// feature; see `EmbeddingBackends::build`.
    Candle,
    /// A local model server's `/api/embeddings`, one request per text.
    Ollama,
    /// Any OpenAI-compatible `/v1/embeddings` endpoint.
    #[serde(rename = "openai")]
    OpenAi,
    /// Deterministic word-overlap, no model, no network. The default: a backend nobody has
    /// finished configuring still resolves to *something* rather than refusing to build at all —
    /// the same reasoning `LimitClass::Unknown` uses for the safe direction on an unclassified
    /// error.
    #[default]
    Hash,
}

impl std::fmt::Display for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Candle => "candle",
            Self::Ollama => "ollama",
            Self::OpenAi => "openai",
            Self::Hash => "hash",
        })
    }
}

/// A backend definition, as stored. The id is the filename, so it cannot drift from what the
/// file is called.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingBackendManifest {
    /// What a human calls it — "Local ollama — nomic-embed-text". Free text; the id is what
    /// everything else refers to.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Which of the four runtimes builds this backend.
    pub runtime: Runtime,
    /// The model this backend embeds with. Every stored vector is recorded against this name —
    /// see "the principle" in `docs/embedding-backends.md` — so it is required, not inferred.
    pub model: String,
    /// The width of the vectors this backend produces. Required for the same reason `model` is:
    /// the same-model failover check compares both, and neither is worth trusting a network call
    /// to learn just to validate a save.
    pub dimensions: u32,
    /// The host (`ollama`) or endpoint base (`openai`). Unused by `candle` and `hash`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The environment variable an `openai` backend's key is read from. A name, not a secret —
    /// mirrors `LlmBackendManifest::api_key_env` exactly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Other backend ids, tried in order if this one's runtime fails to build or fails to
    /// answer. Refused at save time (see [`EmbeddingBackends::save`]) and re-checked at resolve
    /// time if a fallback's model or dimensions no longer match this backend's — see
    /// [`EmbeddingBackends::resolve`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl adi_config::Timestamped for EmbeddingBackendManifest {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

/// A backend definition paired with its filename-derived id.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EmbeddingBackend {
    pub id: String,
    pub manifest: EmbeddingBackendManifest,
}

/// The on-disk store of backend definitions: `embeddings/backends/<id>.toml`.
#[derive(Debug, Clone)]
pub struct EmbeddingBackends {
    config: Config,
}

impl EmbeddingBackends {
    /// Open the store over a config root.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The config root this store reads and writes under — what [`crate::settings`] opens its
    /// own module from, so a caller resolving a consumer only has to hold one of these.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    fn module(&self) -> Module {
        self.config
            .module(&format!("{EMBEDDINGS_MODULE}/{BACKENDS_DIR}"))
    }

    /// Where the definitions live.
    #[must_use]
    pub fn dir(&self) -> std::path::PathBuf {
        self.module().dir().to_path_buf()
    }

    /// Every backend, by id. A store nothing has written to yet is an empty list, not an error.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure, or [`Error::Config`] on an unreadable
    /// definition.
    pub fn list(&self) -> Result<Vec<EmbeddingBackend>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(adi_config::MANIFEST_EXT) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(backend) = self.get(id)? {
                out.push(backend);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// One backend by id, or `None` when there is no such definition.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an id that cannot be a filename, [`Error::Config`] when the
    /// definition exists but cannot be read.
    pub fn get(&self, id: &str) -> Result<Option<EmbeddingBackend>> {
        adi_config::validate_name(id, Error::InvalidName)?;
        let file = self.module().manifest_file::<EmbeddingBackendManifest>(id);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(EmbeddingBackend {
            id: id.to_string(),
            manifest: file.load()?,
        }))
    }

    /// Create or replace a backend definition, stamping the timestamps.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an id that cannot be a filename, [`Error::Arguments`] when the
    /// definition is self-contradictory or names an invalid fallback, or [`Error::Config`] when
    /// it cannot be written.
    pub fn save(&self, id: &str, mut manifest: EmbeddingBackendManifest) -> Result<EmbeddingBackend> {
        adi_config::validate_name(id, Error::InvalidName)?;
        self.validate(id, &manifest)?;
        let now = adi_config::now_unix();
        let existing = self.get(id)?;
        manifest.created_at = existing
            .as_ref()
            .map_or(now, |backend| backend.manifest.created_at);
        manifest.updated_at = now;
        self.module().manifest_file(id).save(&manifest)?;
        Ok(EmbeddingBackend {
            id: id.to_string(),
            manifest,
        })
    }

    /// Delete a backend definition, returning whether it existed.
    ///
    /// The caller is expected to have checked which assignments and fallbacks name it first —
    /// this store knows nothing about either, deliberately, the same division of labour
    /// `LlmBackends::delete` draws against agents.
    ///
    /// # Errors
    /// [`Error::Config`] on a removal failure other than not-found.
    pub fn delete(&self, id: &str) -> Result<bool> {
        adi_config::validate_name(id, Error::InvalidName)?;
        Ok(self.module().remove_manifest(id)?)
    }

    /// Reject a definition that cannot mean what it says: an empty model or width, a fallback
    /// naming itself, a fallback naming no backend at all, or a fallback whose model or
    /// dimensions disagree with this one's. The last of these is the whole point of the store —
    /// see "the same-model failover rule" in `docs/embedding-backends.md`.
    ///
    /// # Errors
    /// [`Error::Arguments`] on any of the above.
    fn validate(&self, id: &str, manifest: &EmbeddingBackendManifest) -> Result<()> {
        if manifest.model.trim().is_empty() {
            return Err(Error::Arguments(
                "a backend needs a model name — every vector it makes is recorded against it".into(),
            ));
        }
        if manifest.dimensions == 0 {
            return Err(Error::Arguments(
                "a backend needs its vector width, `dimensions`".into(),
            ));
        }
        if manifest.runtime == Runtime::Hash
            && (manifest.model != crate::runtimes::hash::MODEL_NAME
                || manifest.dimensions != crate::runtimes::hash::HASH_DIMENSIONS)
        {
            return Err(Error::Arguments(format!(
                "the hash runtime always produces `{}` at {} dimensions — declare that, not a \
                 different name or width",
                crate::runtimes::hash::MODEL_NAME,
                crate::runtimes::hash::HASH_DIMENSIONS
            )));
        }
        #[cfg(feature = "candle")]
        if manifest.runtime == Runtime::Candle
            && (manifest.model != adi_indexer::embed::CANDLE_MODEL_ID
                || manifest.dimensions != adi_indexer::embed::CANDLE_DIMENSIONS)
        {
            return Err(Error::Arguments(format!(
                "the candle runtime always produces `{}` at {} dimensions — declare that, not a \
                 different name or width",
                adi_indexer::embed::CANDLE_MODEL_ID,
                adi_indexer::embed::CANDLE_DIMENSIONS
            )));
        }
        for fallback_id in &manifest.fallbacks {
            if fallback_id == id {
                return Err(Error::Arguments(format!(
                    "`{id}` cannot name itself as its own fallback"
                )));
            }
            let Some(fallback) = self.get(fallback_id)? else {
                return Err(Error::Arguments(format!(
                    "fallback `{fallback_id}` names no such backend"
                )));
            };
            if fallback.manifest.model != manifest.model
                || fallback.manifest.dimensions != manifest.dimensions
            {
                return Err(Error::Arguments(format!(
                    "fallback `{fallback_id}` declares {} at {} dimensions, but `{id}` declares \
                     {} at {} — failover only between backends that would write the same vector \
                     space",
                    fallback.manifest.model,
                    fallback.manifest.dimensions,
                    manifest.model,
                    manifest.dimensions
                )));
            }
        }
        Ok(())
    }

    /// Build the working [`Embedder`] for one backend, following its fallback chain.
    ///
    /// The chain is `[id] + fallbacks`, re-filtered by model and dimensions here rather than
    /// trusted from save time — a fallback may have been edited since. A fallback that no longer
    /// matches is dropped silently; the primary is not, because the primary is what the caller
    /// explicitly asked for, and a primary that cannot be built at all is the error this method
    /// returns.
    ///
    /// # Errors
    /// [`Error::NotFound`] when `id` names no backend. [`Error::Embed`] when every surviving
    /// member of the chain fails to build (including a `candle` backend in a build without the
    /// `candle` feature, which is reported as [`EmbedError::Unavailable`], not fatal to the
    /// store).
    pub fn resolve(&self, id: &str) -> Result<Arc<dyn Embedder>> {
        let primary = self.get(id)?.ok_or_else(|| Error::NotFound(id.to_string()))?;
        let mut chain: Vec<Arc<dyn Embedder>> = Vec::new();
        let mut built_primary = None;
        match self.build(&primary.manifest) {
            Ok(embedder) => {
                built_primary = Some((embedder.dimensions(), embedder.model_name().to_string()));
                chain.push(embedder);
            }
            Err(e) => {
                // Not fatal here — a fallback may still make the chain non-empty. Logged because
                // the operator asked for `id` specifically, and "here is why `id` itself did not
                // work" is worth keeping even when a fallback papers over it.
                tracing::debug!(backend = id, error = %e, "primary embedding backend failed to build");
            }
        }
        for fallback_id in &primary.manifest.fallbacks {
            let Some(fallback) = self.get(fallback_id)? else {
                continue; // deleted since the primary named it; see `validate`'s resolve-time note
            };
            if fallback.manifest.model != primary.manifest.model
                || fallback.manifest.dimensions != primary.manifest.dimensions
            {
                continue; // edited out of alignment since the primary named it — drop it, not the chain
            }
            if let Ok(embedder) = self.build(&fallback.manifest) {
                chain.push(embedder);
            }
        }
        if chain.is_empty() {
            return Err(Error::Embed(EmbedError::Unavailable(format!(
                "embedding backend `{id}` and every one of its fallbacks failed to build"
            ))));
        }
        let (dimensions, model) = built_primary.unwrap_or_else(|| {
            (chain[0].dimensions(), chain[0].model_name().to_string())
        });
        Ok(Arc::new(FailoverEmbedder {
            model,
            dimensions,
            chain,
        }))
    }

    /// Build the concrete [`Embedder`] one manifest describes, with no fallback involved.
    ///
    /// # Errors
    /// [`Error::Embed`] — [`EmbedError::Unavailable`] for a `candle` backend in a build without
    /// the `candle` feature, [`EmbedError::Config`]/[`EmbedError::Embedding`] for a runtime that
    /// failed to construct (a candle model download failure, say).
    fn build(&self, manifest: &EmbeddingBackendManifest) -> Result<Arc<dyn Embedder>> {
        match manifest.runtime {
            Runtime::Candle => build_candle(),
            Runtime::Ollama => {
                let host = manifest.base_url.clone().unwrap_or_default();
                Ok(Arc::new(OllamaEmbedder::new(
                    host,
                    manifest.model.clone(),
                    manifest.dimensions,
                )))
            }
            Runtime::OpenAi => {
                let base_url = manifest.base_url.clone().unwrap_or_default();
                Ok(Arc::new(OpenAiEmbedder::new(
                    base_url,
                    manifest.model.clone(),
                    manifest.dimensions,
                    manifest.api_key_env.clone(),
                )))
            }
            Runtime::Hash => Ok(Arc::new(HashEmbedder)),
        }
    }
}

#[cfg(feature = "candle")]
fn build_candle() -> Result<Arc<dyn Embedder>> {
    Ok(Arc::new(adi_indexer::embed::CandleEmbedder::new()?))
}

#[cfg(not(feature = "candle"))]
fn build_candle() -> Result<Arc<dyn Embedder>> {
    Err(Error::Embed(EmbedError::Unavailable(
        "this build has no candle embedder — rebuild adi-embeddings with the `candle` feature \
         for the `candle` runtime"
            .to_string(),
    )))
}

/// The [`Embedder`] a resolved chain hands back: tries each member in order on every call and
/// returns the first success, so a runtime failure (an unreachable ollama, a dropped connection
/// to a hosted endpoint) fails over per call, not only at start-up.
///
/// `model_name`/`dimensions` answer with the primary's — every member of the chain is validated
/// to share both, so any member would answer the same.
#[derive(Debug)]
struct FailoverEmbedder {
    model: String,
    dimensions: u32,
    chain: Vec<Arc<dyn Embedder>>,
}

impl Embedder for FailoverEmbedder {
    fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        let mut last = None;
        for embedder in &self.chain {
            match embedder.embed(texts) {
                Ok(vectors) => return Ok(vectors),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| EmbedError::Unavailable("empty failover chain".into())))
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

    fn scratch(tag: &str) -> EmbeddingBackends {
        let root = std::env::temp_dir().join(format!(
            "adi-embeddings-backend-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        EmbeddingBackends::with_config(Config::with_root(root))
    }

    fn ollama_manifest() -> EmbeddingBackendManifest {
        EmbeddingBackendManifest {
            label: "Local ollama".into(),
            runtime: Runtime::Ollama,
            model: "nomic-embed-text".into(),
            dimensions: 768,
            base_url: Some("http://127.0.0.1:11434".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_definition_round_trips_through_the_store() {
        let store = scratch("round-trip");
        store.save("ollama", ollama_manifest()).expect("save");

        let read = store.get("ollama").expect("get").expect("present");
        assert_eq!(read.id, "ollama");
        assert_eq!(read.manifest.model, "nomic-embed-text");
        assert_eq!(read.manifest.runtime, Runtime::Ollama);
        assert_eq!(read.manifest.dimensions, 768);
        assert!(read.manifest.created_at > 0, "timestamps are stamped");
    }

    #[test]
    fn the_file_carries_no_id_of_its_own() {
        let text = toml::to_string_pretty(&ollama_manifest()).expect("toml");
        assert!(!text.contains("id ="), "{text}");
    }

    #[test]
    fn listing_is_sorted_and_an_empty_store_is_not_an_error() {
        let store = scratch("list");
        assert!(store.list().expect("empty list").is_empty());

        store
            .save(
                "hash",
                EmbeddingBackendManifest {
                    runtime: Runtime::Hash,
                    model: "hash-bow-256".into(),
                    dimensions: 256,
                    ..Default::default()
                },
            )
            .expect("save hash");
        store.save("ollama", ollama_manifest()).expect("save ollama");

        let ids: Vec<String> = store.list().expect("list").into_iter().map(|b| b.id).collect();
        assert_eq!(ids, vec!["hash".to_string(), "ollama".to_string()]);
    }

    #[test]
    fn saving_twice_keeps_the_original_created_at() {
        let store = scratch("created-at");
        let first = store.save("ollama", ollama_manifest()).expect("save");
        let again = store.save("ollama", ollama_manifest()).expect("save again");
        assert_eq!(again.manifest.created_at, first.manifest.created_at);
    }

    #[test]
    fn deleting_reports_whether_it_was_there() {
        let store = scratch("delete");
        store.save("ollama", ollama_manifest()).expect("save");
        assert!(store.delete("ollama").expect("delete"));
        assert!(!store.delete("ollama").expect("delete again"));
        assert!(store.get("ollama").expect("get").is_none());
    }

    #[test]
    fn a_backend_with_no_model_is_refused() {
        let store = scratch("validate-model");
        let manifest = EmbeddingBackendManifest {
            dimensions: 768,
            ..Default::default()
        };
        let err = store.save("nowhere", manifest).expect_err("refused");
        assert!(err.to_string().contains("model"), "{err}");
    }

    #[test]
    fn a_backend_with_no_dimensions_is_refused() {
        let store = scratch("validate-dimensions");
        let manifest = EmbeddingBackendManifest {
            model: "x".into(),
            ..Default::default()
        };
        assert!(store.save("nowhere", manifest).is_err());
    }

    /// The accept case: two backends declaring the same model and width may fail over between
    /// each other.
    #[test]
    fn a_same_model_fallback_is_accepted() {
        let store = scratch("failover-accept");
        store.save("ollama-local", ollama_manifest()).expect("save primary base");
        let primary = EmbeddingBackendManifest {
            fallbacks: vec!["ollama-local".into()],
            ..ollama_manifest()
        };
        store.save("ollama-hosted", primary).expect("accepted");
    }

    /// The refuse case: a fallback naming a different model, or the same model at a different
    /// width, must not be nameable — see "the same-model failover rule" in
    /// `docs/embedding-backends.md`.
    #[test]
    fn a_different_model_fallback_is_refused() {
        let store = scratch("failover-refuse-model");
        store
            .save(
                "candle",
                EmbeddingBackendManifest {
                    runtime: Runtime::Candle,
                    model: "jinaai/jina-embeddings-v2-base-code".into(),
                    dimensions: 768,
                    ..Default::default()
                },
            )
            .expect("save candle");
        let primary = EmbeddingBackendManifest {
            fallbacks: vec!["candle".into()],
            ..ollama_manifest()
        };
        let err = store.save("ollama-hosted", primary).expect_err("refused");
        assert!(err.to_string().contains("vector space"), "{err}");
    }

    #[test]
    fn a_different_width_fallback_is_refused_even_at_the_same_model_name() {
        let store = scratch("failover-refuse-width");
        store
            .save(
                "ollama-narrow",
                EmbeddingBackendManifest {
                    dimensions: 384,
                    ..ollama_manifest()
                },
            )
            .expect("save narrow");
        let primary = EmbeddingBackendManifest {
            fallbacks: vec!["ollama-narrow".into()],
            ..ollama_manifest()
        };
        assert!(store.save("ollama-wide", primary).is_err());
    }

    #[test]
    fn a_backend_cannot_name_itself_as_its_own_fallback() {
        let store = scratch("failover-self");
        let manifest = EmbeddingBackendManifest {
            fallbacks: vec!["self-loop".into()],
            ..ollama_manifest()
        };
        assert!(store.save("self-loop", manifest).is_err());
    }

    #[test]
    fn a_fallback_naming_nothing_is_refused() {
        let store = scratch("failover-missing");
        let manifest = EmbeddingBackendManifest {
            fallbacks: vec!["nowhere".into()],
            ..ollama_manifest()
        };
        assert!(store.save("ollama", manifest).is_err());
    }

    /// Resolving the `hash` runtime never touches a network — it is the one runtime every build
    /// can always resolve.
    #[test]
    fn resolving_a_hash_backend_always_succeeds() {
        let store = scratch("resolve-hash");
        store
            .save(
                "hash",
                EmbeddingBackendManifest {
                    runtime: Runtime::Hash,
                    model: "hash-bow-256".into(),
                    dimensions: 256,
                    ..Default::default()
                },
            )
            .expect("save");
        let embedder = store.resolve("hash").expect("resolve");
        assert_eq!(embedder.model_name(), "hash-bow-256");
        assert_eq!(embedder.embed(&["a"]).expect("embed").len(), 1);
    }

    #[test]
    fn resolving_an_unknown_id_is_not_found() {
        let store = scratch("resolve-missing");
        assert!(store.resolve("nowhere").is_err());
    }

    /// A stale fallback — edited out of alignment since the primary named it — is dropped at
    /// resolve time rather than trusted from save time. `ollama` is used here rather than
    /// `hash`: its width is an operator-declared field with nothing to check it against, so a
    /// second, independent `save` of the fallback backend can legitimately drift its dimensions
    /// away from what `primary` recorded when it named it — exactly the scenario "the same-model
    /// failover rule" describes as needing a *second* check, at resolve time.
    #[test]
    fn a_fallback_edited_out_of_alignment_is_dropped_at_resolve_time() {
        let store = scratch("resolve-stale-fallback");
        store
            .save("fallback", ollama_manifest())
            .expect("save fallback base, 768 dimensions");
        store
            .save(
                "primary",
                EmbeddingBackendManifest {
                    fallbacks: vec!["fallback".into()],
                    ..ollama_manifest()
                },
            )
            .expect("save primary, fallback matches at save time");
        // An independent, later edit of the fallback backend — nothing about `primary` changes,
        // and nothing re-checks `primary`'s fallback list when this save happens.
        store
            .save(
                "fallback",
                EmbeddingBackendManifest {
                    dimensions: 384, // no longer matches `primary`
                    ..ollama_manifest()
                },
            )
            .expect("save edited fallback in isolation");
        let embedder = store
            .resolve("primary")
            .expect("primary still resolves on its own");
        assert_eq!(
            embedder.dimensions(),
            768,
            "the primary's own width, not the stale fallback's"
        );
    }
}
