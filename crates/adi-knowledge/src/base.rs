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

/// The on-disk registry of bases for one store module: where a base's directory is, what its
/// manifest says, and which bases exist.
///
/// Split out of [`KnowledgeStore`](crate::KnowledgeStore) because `adi-facts` addresses its bases
/// by exactly the same [`BaseId`] at exactly the same three levels, and everything about that
/// layout — `global/<base>/`, `projects/<id>/<base>/`, `agents/<name>/<base>/`, each with a
/// `base.toml` — is common to both. Only what the directory *contains* differs.
///
/// It answers no access question. Who may see a base is a [`Reader`](crate::Reader)'s business,
/// and keeping the two apart is what stops a caller from getting a listing that quietly enforced
/// somebody else's rules.
#[derive(Debug, Clone)]
pub struct BaseRegistry {
    config: adi_config::Config,
    module: String,
}

/// The manifest inside each base's directory.
const BASE_MANIFEST: &str = "base.toml";

impl BaseRegistry {
    /// A registry over `<store root>/<module>` — `knowledge` or `facts`.
    #[must_use]
    pub fn new(config: adi_config::Config, module: impl Into<String>) -> Self {
        Self {
            config,
            module: module.into(),
        }
    }

    /// The store this reads from.
    #[must_use]
    pub fn config(&self) -> &adi_config::Config {
        &self.config
    }

    /// The module's directory: `~/.adi/mono/<module>`.
    #[must_use]
    pub fn dir(&self) -> std::path::PathBuf {
        self.config.module(&self.module).dir().to_path_buf()
    }

    /// Where one base's files live.
    #[must_use]
    pub fn base_dir(&self, id: &BaseId) -> std::path::PathBuf {
        self.dir().join(id.rel_dir())
    }

    /// One base's manifest file, whether or not it exists.
    #[must_use]
    pub fn manifest_file(&self, id: &BaseId) -> adi_config::ConfigFile<BaseManifest> {
        self.config
            .module(&self.module)
            .file(&format!("{}/{BASE_MANIFEST}", id.rel_dir()))
    }

    /// Whether a base has been created.
    #[must_use]
    pub fn exists(&self, id: &BaseId) -> bool {
        self.manifest_file(id).exists()
    }

    /// One base, or `None` if it is not there. **No access check** — see the type's docs.
    ///
    /// # Errors
    /// A config error when the manifest is there but will not parse.
    pub fn load(&self, id: &BaseId) -> crate::Result<Option<Base>> {
        let file = self.manifest_file(id);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(Base {
            id: id.clone(),
            manifest: file.load()?,
        }))
    }

    /// Write a base's manifest, creating its directory.
    ///
    /// # Errors
    /// A config error when the file cannot be written.
    pub fn save(&self, id: &BaseId, manifest: &BaseManifest) -> crate::Result<()> {
        self.manifest_file(id).save(manifest)?;
        Ok(())
    }

    /// Remove a base's directory and everything under it. `false` if it wasn't there.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) when the directory cannot be removed.
    pub fn remove(&self, id: &BaseId) -> crate::Result<bool> {
        let dir = self.base_dir(id);
        if !dir.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dir)?;
        Ok(true)
    }

    /// Every base id on disk, whether or not any given reader may see it.
    #[must_use]
    pub fn scan(&self) -> Vec<BaseId> {
        let root = self.dir();
        let mut out = Vec::new();
        // The three levels are three directory shapes: `global/<base>`, and one more level of
        // owner under each of the other two. `Scope::rel_dir` writes these; this reads them.
        collect_bases(&root.join("global"), &crate::Scope::Global, &mut out);
        for owner in child_names(&root.join("projects")) {
            if let Ok(scope) = crate::Scope::project(owner.clone()) {
                collect_bases(&root.join("projects").join(&owner), &scope, &mut out);
            }
        }
        for owner in child_names(&root.join("agents")) {
            if let Ok(scope) = crate::Scope::agent(owner.clone()) {
                collect_bases(&root.join("agents").join(&owner), &scope, &mut out);
            }
        }
        out
    }
}

/// Directory names directly under `dir` that could name a scope owner or a base, sorted.
#[must_use]
pub fn child_names(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| crate::scope::validate_segment(name).is_ok())
        .collect();
    out.sort();
    out
}

/// Append every base directly under `dir` as a base of `scope`.
fn collect_bases(dir: &std::path::Path, scope: &crate::Scope, out: &mut Vec<BaseId>) {
    for name in child_names(dir) {
        if !dir.join(&name).join(BASE_MANIFEST).exists() {
            continue;
        }
        if let Ok(id) = BaseId::new(scope.clone(), name) {
            out.push(id);
        }
    }
}
