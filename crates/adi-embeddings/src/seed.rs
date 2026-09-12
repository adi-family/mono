//! Materializing the backends and assignments that reproduce today's hardcoded behaviour, so
//! that adopting this registry changes nothing until an operator asks it to.
//!
//! Runs once. `embeddings/settings.toml`'s *presence* is the marker — the same shape a
//! migration's version stamp is (`docs/llm-backends.md`'s "The version stamp") — not "the store
//! currently has zero backends," so a store an operator has since emptied on purpose is never
//! re-seeded.

use adi_config::Module;

use crate::backend::{EmbeddingBackendManifest, EmbeddingBackends, Runtime};
use crate::error::Result;
use crate::settings::{EmbeddingSettings, SETTINGS_FILE};
use crate::{CONSUMER_FACTS, CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, EMBEDDINGS_MODULE};

/// The id every seed writes the `candle` backend under.
const CANDLE_ID: &str = "candle";
/// The id every seed writes the `ollama` backend under.
const OLLAMA_ID: &str = "ollama";

/// The environment variable `adi_facts::embed::OllamaEmbedder` reads its host from, honoured
/// here **only at seed time** — see `docs/embedding-backends.md`'s "Seeding".
const ADI_FACTS_OLLAMA: &str = "ADI_FACTS_OLLAMA";
/// The environment variable `adi_facts::embed::OllamaEmbedder` reads its model from, honoured
/// here only at seed time.
const ADI_FACTS_EMBED: &str = "ADI_FACTS_EMBED";

/// `adi_facts::ollama::Ollama`'s default host — duplicated as a literal rather than imported,
/// the same call this module makes for the candle model/width below.
const DEFAULT_OLLAMA_HOST: &str = "http://127.0.0.1:11434";
/// `adi_facts::embed::OllamaEmbedder`'s default model.
const DEFAULT_OLLAMA_MODEL: &str = "nomic-embed-text";
/// nomic-embed-text's declared width, matching `adi_facts::embed`'s own constant.
const DEFAULT_OLLAMA_DIMENSIONS: u32 = 768;

/// Read the assignments, seeding the store first if it has never been seeded.
///
/// # Errors
/// Whatever [`EmbeddingBackends::save`] or [`EmbeddingSettings::save`]/[`EmbeddingSettings::load`]
/// return — a store that cannot be written to is the one failure this cannot paper over.
pub(crate) fn seed_if_needed(backends: &EmbeddingBackends) -> Result<EmbeddingSettings> {
    let module: Module = backends.config().module(EMBEDDINGS_MODULE);
    let settings_file = module.file::<EmbeddingSettings>(SETTINGS_FILE);
    if settings_file.exists() {
        return EmbeddingSettings::load(&module);
    }

    // A literal, not `adi_indexer::embed::CANDLE_MODEL_ID`/`CANDLE_DIMENSIONS`: seeding must
    // write the same manifest whether or not this binary was built with the `candle` feature, so
    // the backend is there — reported unavailable, not missing — the day the feature is turned
    // back on. See `docs/embedding-backends.md`'s "Seeding".
    backends.save(
        CANDLE_ID,
        EmbeddingBackendManifest {
            label: "Local code embeddings (candle)".into(),
            runtime: Runtime::Candle,
            model: "jinaai/jina-embeddings-v2-base-code".into(),
            dimensions: 768,
            ..Default::default()
        },
    )?;

    backends.save(
        OLLAMA_ID,
        EmbeddingBackendManifest {
            label: "Local ollama embeddings".into(),
            runtime: Runtime::Ollama,
            model: env_or(ADI_FACTS_EMBED, DEFAULT_OLLAMA_MODEL),
            dimensions: DEFAULT_OLLAMA_DIMENSIONS,
            base_url: Some(env_or(ADI_FACTS_OLLAMA, DEFAULT_OLLAMA_HOST)),
            ..Default::default()
        },
    )?;

    let mut settings = EmbeddingSettings::default();
    settings
        .assignments
        .insert(CONSUMER_INDEXER.to_string(), CANDLE_ID.to_string());
    settings
        .assignments
        .insert(CONSUMER_KNOWLEDGE.to_string(), CANDLE_ID.to_string());
    settings
        .assignments
        .insert(CONSUMER_FACTS.to_string(), OLLAMA_ID.to_string());
    settings.save(&module)?;
    Ok(settings)
}

/// An environment variable, treating blank as unset — the same rule
/// `adi_facts::ollama::env_or` applies, duplicated rather than imported because `adi-facts`
/// cannot be a dependency of this crate (it will depend on this one, in phase B).
fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;

    fn scratch(tag: &str) -> EmbeddingBackends {
        let root = std::env::temp_dir().join(format!(
            "adi-embeddings-seed-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        EmbeddingBackends::with_config(Config::with_root(root))
    }

    #[test]
    fn seeding_writes_the_two_backends_and_the_three_assignments() {
        // Forced rather than trusted from the ambient environment: this machine may itself run
        // `adi-facts` with `ADI_FACTS_OLLAMA`/`ADI_FACTS_EMBED` set, and the point of this test is
        // what seeding does with them, not what happens to be exported in this shell.
        unsafe {
            std::env::set_var(ADI_FACTS_OLLAMA, DEFAULT_OLLAMA_HOST);
            std::env::set_var(ADI_FACTS_EMBED, DEFAULT_OLLAMA_MODEL);
        }
        let backends = scratch("materialize");
        let settings = seed_if_needed(&backends).expect("seed");

        let candle = backends.get(CANDLE_ID).expect("get").expect("present");
        assert_eq!(candle.manifest.runtime, Runtime::Candle);
        assert_eq!(candle.manifest.model, "jinaai/jina-embeddings-v2-base-code");
        assert_eq!(candle.manifest.dimensions, 768);

        let ollama = backends.get(OLLAMA_ID).expect("get").expect("present");
        assert_eq!(ollama.manifest.runtime, Runtime::Ollama);
        assert_eq!(ollama.manifest.model, DEFAULT_OLLAMA_MODEL);
        assert_eq!(
            ollama.manifest.base_url.as_deref(),
            Some(DEFAULT_OLLAMA_HOST)
        );

        assert_eq!(settings.assignments[CONSUMER_INDEXER], CANDLE_ID);
        assert_eq!(settings.assignments[CONSUMER_KNOWLEDGE], CANDLE_ID);
        assert_eq!(settings.assignments[CONSUMER_FACTS], OLLAMA_ID);

        // Same test, not a second `#[test]`: both mutate the same process-global
        // `ADI_FACTS_OLLAMA`/`ADI_FACTS_EMBED`, and cargo runs tests in parallel by default — one
        // function keeps the two scenarios from racing each other.
        unsafe {
            std::env::set_var(ADI_FACTS_OLLAMA, "http://box:11434");
            std::env::set_var(ADI_FACTS_EMBED, "mxbai-embed-large");
        }
        let overridden = scratch("materialize-overridden");
        seed_if_needed(&overridden).expect("seed with overrides set");
        let ollama = overridden.get(OLLAMA_ID).expect("get").expect("present");
        assert_eq!(ollama.manifest.model, "mxbai-embed-large");
        assert_eq!(ollama.manifest.base_url.as_deref(), Some("http://box:11434"));
    }

    #[test]
    fn seeding_runs_once_even_if_the_store_is_later_emptied() {
        let backends = scratch("once");
        seed_if_needed(&backends).expect("first seed");
        assert!(backends.delete(CANDLE_ID).expect("delete"));
        assert!(backends.delete(OLLAMA_ID).expect("delete"));

        let settings = seed_if_needed(&backends).expect("second call");
        assert!(
            backends.get(CANDLE_ID).expect("get").is_none(),
            "an operator's deliberate deletion must not be undone by a later resolve"
        );
        assert_eq!(settings.assignments[CONSUMER_FACTS], OLLAMA_ID);
    }
}
