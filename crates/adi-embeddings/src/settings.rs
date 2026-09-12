//! The assignment: which backend each consumer embeds through, at `embeddings/settings.toml`.
//!
//! ```toml
//! [assignments]
//! indexer   = "candle"
//! knowledge = "candle"
//! facts     = "ollama"
//! ```
//!
//! Unlike `llm/settings.toml` (`LlmSettings::load`), reading this file never writes it: the file's
//! *presence* is [`crate::seed`]'s one-shot marker for "has this store ever been seeded", so a
//! read that materialized an empty default on first touch would make every store look already
//! seeded before it ever was. Only `seed::seed_if_needed` creates this file for the first time; a
//! plain read of an absent one answers with empty assignments and nothing on disk changes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use adi_config::Module;

use crate::error::Result;
use crate::EMBEDDINGS_MODULE;

/// The file the assignments live in, within the [`embeddings`](crate) module.
pub(crate) const SETTINGS_FILE: &str = "settings.toml";

/// The `embeddings/settings.toml` shape. Unknown fields are ignored, so an older store keeps
/// loading.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingSettings {
    /// Which backend id each consumer resolves through. A consumer with no row here is
    /// unassigned, and [`crate::backend::EmbeddingBackends::resolve`]'s caller gets
    /// [`crate::Error::Unassigned`] rather than a silent default to whatever backend happens to
    /// exist.
    pub assignments: BTreeMap<String, String>,
}

impl EmbeddingSettings {
    /// Read the assignments, or an empty set if the store has never written any. Never writes —
    /// see this module's own doc for why.
    ///
    /// # Errors
    /// [`crate::Error::Config`] if the file exists but cannot be parsed as TOML — an operator's
    /// typo must not be silently swallowed into "nothing is assigned."
    pub fn load(module: &Module) -> Result<Self> {
        Ok(module.file::<Self>(SETTINGS_FILE).load_or_default()?)
    }

    /// Read them from a config root.
    ///
    /// # Errors
    /// See [`Self::load`].
    pub fn open(config: &adi_config::Config) -> Result<Self> {
        Self::load(&config.module(EMBEDDINGS_MODULE))
    }

    /// Write them back.
    ///
    /// # Errors
    /// [`crate::Error::Config`] if the file cannot be written.
    pub fn save(&self, module: &Module) -> Result<()> {
        module.file(SETTINGS_FILE).save(self)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-embeddings-settings-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root).module(EMBEDDINGS_MODULE)
    }

    #[test]
    fn reading_an_absent_file_answers_empty_and_writes_nothing() {
        let module = scratch("absent");
        let settings = EmbeddingSettings::load(&module).expect("load");
        assert!(settings.assignments.is_empty());
        assert!(
            !module.dir().join(SETTINGS_FILE).exists(),
            "a read must not be the thing that seeds the store"
        );
    }

    #[test]
    fn settings_round_trip() {
        let module = scratch("round-trip");
        let mut settings = EmbeddingSettings::default();
        settings
            .assignments
            .insert("knowledge".into(), "candle".into());
        settings.save(&module).expect("save");

        let read = EmbeddingSettings::load(&module).expect("load");
        assert_eq!(read.assignments.get("knowledge"), Some(&"candle".to_string()));
    }

    #[test]
    fn a_corrupt_file_is_reported_rather_than_read_as_empty() {
        let module = scratch("corrupt");
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(SETTINGS_FILE), "not = [toml").expect("write");
        assert!(EmbeddingSettings::load(&module).is_err());
    }
}
