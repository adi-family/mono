//! A knowledge base: the manifest on disk, and the view a caller gets back.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::scope::BaseId;

/// What a base's `base.toml` holds. The notes themselves belong to the provider.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BaseManifest {
    /// Which [provider](crate::backend::Provider) holds this base's notes.
    pub provider: String,
    /// A line about what belongs in here, for whoever finds it later.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Provider-specific settings, passed through untouched — a connection string, a collection
    /// name, whatever a given provider asked for.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds.
    pub updated_at: u64,
}

impl adi_config::Timestamped for BaseManifest {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

/// A base, paired with the id its location gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    /// Where it sits: scope plus name.
    pub id: BaseId,
    /// Its manifest.
    #[serde(flatten)]
    pub manifest: BaseManifest,
}

impl Base {
    /// The isolation level — `global`, `project`, or `agent`.
    #[must_use]
    pub fn level(&self) -> &'static str {
        self.id.scope.level()
    }

    /// Whether this base is an agent's own memory.
    #[must_use]
    pub fn is_memory(&self) -> bool {
        self.id.is_memory()
    }
}

/// What a base holds, and how much of it is currently searchable by meaning.
///
/// `stale` is the number worth watching: a note counts as stale when its text has moved since it
/// was embedded, or when it was embedded by a different model. Anything stale is findable by
/// word and not by meaning until `reembed` catches up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaseStatus {
    /// Which base this describes.
    pub base: Base,
    /// How many notes it holds.
    pub notes: usize,
    /// How many of them have current vectors.
    pub embedded: usize,
    /// How many need embedding or re-embedding.
    pub stale: usize,
    /// The model those counts were judged against, if one is loaded or configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::Scope;

    #[test]
    fn a_base_reports_its_level_and_whether_it_is_an_agents_memory() {
        let memory = Base {
            id: BaseId::memory("solver").unwrap(),
            manifest: BaseManifest::default(),
        };
        assert_eq!(memory.level(), "agent");
        assert!(memory.is_memory());

        let named = Base {
            id: BaseId::new(Scope::agent("solver").unwrap(), "scratch").unwrap(),
            manifest: BaseManifest::default(),
        };
        assert_eq!(named.level(), "agent");
        assert!(!named.is_memory(), "only `memory` is the memory base");
    }

    /// The manifest is TOML on disk; a round trip through it must not lose provider settings.
    #[test]
    fn a_manifest_round_trips_through_toml() {
        let manifest = BaseManifest {
            provider: "sqlite".into(),
            description: Some("runbooks".into()),
            settings: BTreeMap::from([("collection".to_string(), "ops".to_string())]),
            created_at: 10,
            updated_at: 20,
        };
        let text = toml::to_string(&manifest).expect("serialize");
        let back: BaseManifest = toml::from_str(&text).expect("parse");
        assert_eq!(back, manifest);
    }
}
