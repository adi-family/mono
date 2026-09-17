//! The embedding backend registry — a named, configurable set of ways to turn text into a
//! vector, parallel to `adi_agents::llm`'s LLM backend registry.
//!
//! A consumer (`indexer`, `knowledge`, `facts`) names an assignment. An assignment names a
//! backend. A backend is one complete way to embed — a [`Runtime`], a model,
//! and the dials it runs with, under a name a human chose. The whole design is
//! `docs/embedding-backends.md`; the short version is two layers and no third:
//!
//! ```text
//! CONSUMER    indexer · knowledge · facts                    which store is asking?
//! ASSIGNMENT  knowledge -> "candle"                           settings.toml
//! BACKEND     runtime · model · dimensions · fallbacks        embeddings/backends/<id>.toml
//! ```
//!
//! What lives where:
//!
//! * [`backend`] — the backend definition, its on-disk store, and resolving one backend (plus its
//!   validated same-model fallback chain) into a concrete [`Embedder`].
//! * [`settings`] — `embeddings/settings.toml`, the consumer → backend assignments.
//! * [`seed`] — materializing the backends and assignments that reproduce today's hardcoded
//!   behaviour, the first time anything resolves against a store that has never been seeded.
//! * [`runtimes`] — the four ways a backend can actually turn text into a vector: `candle`
//!   (dispatched directly to [`adi_indexer::embed::CandleEmbedder`] from [`backend`]), and
//!   [`runtimes::ollama`], [`runtimes::openai`], [`runtimes::hash`].
//! * [`ondemand`] — the human-triggered test: embed one short string through a backend (saved or
//!   still a draft) and report success, width and latency, right now.
//!
//! **Phase B is built.** `adi-knowledge` and `adi-facts` call [`resolve`] from
//! `KnowledgeStore::with_config`/`FactStore::with_config`; `adi-indexer` cannot call it itself
//! (this crate depends on `adi-indexer`, so the reverse edge would be a cycle) and instead has
//! its one production caller, `adi-cli`'s `indexer` command group, resolve on its behalf and
//! hand the result to `Indexer::open_with_embedder`. What phase C still owes is the operator
//! surface: a CLI, an API, and a panel tab. See `docs/embedding-backends.md`.

pub mod backend;
pub mod ondemand;
pub mod runtimes;
pub mod seed;
pub mod settings;

mod error;

use std::sync::Arc;

use adi_config::Config;
use adi_indexer::embed::Embedder;

pub use backend::{
    CANDLE_FIXED_MODEL, EmbeddingBackend, EmbeddingBackendManifest, EmbeddingBackends, Runtime,
};
pub use error::{Error, Result};
pub use ondemand::{TestResult, TestVerdict, test_backend, test_manifest};
pub use runtimes::hash::HashEmbedder;
pub use runtimes::ollama::OllamaEmbedder;
pub use runtimes::openai::OpenAiEmbedder;
pub use settings::EmbeddingSettings;

/// The store module every embedding-backend file lives under: `embeddings/` in the mono store.
pub const EMBEDDINGS_MODULE: &str = "embeddings";

/// The subdirectory of [`EMBEDDINGS_MODULE`] holding one `<id>.toml` per backend.
pub const BACKENDS_DIR: &str = "backends";

/// The consumer name `adi-indexer` resolves under, once phase B wires it in.
pub const CONSUMER_INDEXER: &str = "indexer";
/// The consumer name `adi-knowledge` resolves under.
pub const CONSUMER_KNOWLEDGE: &str = "knowledge";
/// The consumer name `adi-facts` resolves under.
pub const CONSUMER_FACTS: &str = "facts";

/// Resolve a consumer's embedder: seed the store if it has never been seeded, look up the
/// consumer's assignment, and build the assigned backend's (validated, same-model) fallback
/// chain into one working [`Embedder`].
///
/// # Errors
/// [`Error::Unassigned`] if `consumer` names no row in `embeddings/settings.toml`.
/// [`Error::NotFound`] if the assignment names a backend that no longer exists.
/// [`Error::Embed`] if every backend in the resolved chain fails to build.
pub fn resolve(config: &Config, consumer: &str) -> Result<Arc<dyn Embedder>> {
    let backends = EmbeddingBackends::with_config(config.clone());
    let settings = seed::seed_if_needed(&backends)?;
    let id = settings
        .assignments
        .get(consumer)
        .ok_or_else(|| Error::Unassigned(consumer.to_string()))?;
    backends.resolve(id)
}

/// Materialize the backends and assignments that reproduce today's hardcoded behaviour, if this
/// store has never been seeded — the explicit form of what [`resolve`] otherwise does lazily on its
/// first call. For an operator (`adi-mono embeddings seed`) who wants a fresh store's effective
/// defaults to show up before anything has actually embedded through it, without [`resolve`]'s
/// cost of going on to *build* one — a `candle` build is a model load, and this must stay cheap
/// enough to run from a page load.
///
/// # Errors
/// Whatever [`EmbeddingBackends::save`] or [`EmbeddingSettings::save`]/[`EmbeddingSettings::load`]
/// return — a store that cannot be written to is the one failure this cannot paper over.
pub fn ensure_seeded(config: &Config) -> Result<EmbeddingSettings> {
    seed::seed_if_needed(&EmbeddingBackends::with_config(config.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-embeddings-lib-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    /// A fresh store answers every one of today's three consumers without any setup — seeding
    /// happens transparently on the first call.
    ///
    /// Only `facts` (the `ollama` runtime) is resolved all the way to a built [`Embedder`] here:
    /// `indexer`/`knowledge` seed to `candle`, and *constructing* [`adi_indexer::embed::
    /// CandleEmbedder`] downloads model weights on first use — exactly the cost
    /// `docs/embedding-backends-survey.md` §7 flags as synchronous and network-bound, wrong to
    /// pay inside a unit test. Checking the assignment exists proves seeding ran; resolving
    /// `ollama` (pure construction, no I/O until `embed()` is called) proves `resolve` actually
    /// walks consumer → assignment → backend → `Embedder`.
    #[test]
    fn resolving_a_never_seeded_store_falls_back_to_the_seeded_defaults() {
        let config = scratch("first-resolve");
        let backends = EmbeddingBackends::with_config(config.clone());
        let settings = seed::seed_if_needed(&backends).expect("seed");
        for consumer in [CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, CONSUMER_FACTS] {
            assert!(
                settings.assignments.contains_key(consumer),
                "{consumer} is unassigned after seeding"
            );
        }
        let assigned_id = settings.assignments[CONSUMER_FACTS].clone();
        let assigned = backends.get(&assigned_id).expect("get").expect("present");
        let embedder = resolve(&config, CONSUMER_FACTS).expect("ollama builds without a network call");
        assert_eq!(embedder.model_name(), assigned.manifest.model);
    }

    #[test]
    fn an_unassigned_consumer_is_reported_by_name() {
        let config = scratch("unassigned");
        let err = resolve(&config, "no-such-consumer").expect_err("refused");
        assert!(err.to_string().contains("no-such-consumer"), "{err}");
    }

    /// The explicit seed materializes the same defaults `resolve` would lazily produce, without
    /// building anything — so it never touches candle's model download, unlike the test above.
    #[test]
    fn seeding_explicitly_does_the_same_thing_resolve_would_do_lazily() {
        let config = scratch("explicit-seed");
        let settings = ensure_seeded(&config).expect("seed");
        for consumer in [CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, CONSUMER_FACTS] {
            assert!(settings.assignments.contains_key(consumer));
        }
        assert!(!EmbeddingBackends::with_config(config).list().expect("list").is_empty());
    }
}
