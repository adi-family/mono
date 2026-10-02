// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::embed::EmbeddingConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub embedding: EmbeddingConfig,
    pub parser: ParserConfig,
    pub storage: StorageConfig,
    pub index: IndexConfig,
    pub ignore: IgnoreConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParserConfig {
    pub max_file_size: u64,
    pub enabled_languages: Vec<String>,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            max_file_size: 1024 * 1024, // 1MB
            enabled_languages: vec![],  // Empty = all supported
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub backend: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: "sqlite".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    pub hnsw_m: usize,
    pub hnsw_ef_construction: usize,
    pub hnsw_ef_search: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            hnsw_m: 16,
            hnsw_ef_construction: 200,
            hnsw_ef_search: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IgnoreConfig {
    pub patterns: Vec<String>,
    pub use_gitignore: bool,
    pub use_ignore_file: bool,
}

impl Default for IgnoreConfig {
    fn default() -> Self {
        Self {
            patterns: vec![
                "target".to_string(),
                "node_modules".to_string(),
                ".git".to_string(),
                "__pycache__".to_string(),
                "*.pyc".to_string(),
                ".venv".to_string(),
                "venv".to_string(),
                "dist".to_string(),
                "build".to_string(),
                ".adi".to_string(),
            ],
            use_gitignore: true,
            use_ignore_file: true,
        }
    }
}

impl Config {
    pub fn load(project_path: &Path) -> Result<Self> {
        // User-level defaults from the indexer's module in the mono store.
        let user_config_path = Self::user_config_path();
        // Load project-level config from .adi/config.toml
        let project_config_path = project_path.join(".adi/config.toml");
        Self::load_layers(&[&user_config_path, &project_config_path])
    }

    pub(crate) fn load_layers(paths: &[&Path]) -> Result<Self> {
        let mut config =
            toml::Value::try_from(Self::default()).map_err(|e| Error::Config(e.to_string()))?;
        for path in paths {
            let content = match std::fs::read_to_string(path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let layer: toml::Value = toml::from_str(&content)?;
            // Validate each file before merging so a later layer cannot hide invalid types.
            let _: Self = layer.clone().try_into()?;
            Self::merge_layer(&mut config, layer);
        }
        Ok(config.try_into()?)
    }

    pub fn save_project(&self, project_path: &Path) -> Result<()> {
        let config_dir = project_path.join(".adi");
        let config_path = config_dir.join("config.toml");
        let content = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::create_dir_all(config_dir)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    #[must_use]
    pub fn user_dir() -> PathBuf {
        crate::paths::module_dir()
    }

    #[must_use]
    pub fn user_config_path() -> PathBuf {
        crate::paths::user_config_path()
    }

    fn merge_layer(config: &mut toml::Value, mut layer: toml::Value) {
        // Custom ignore rules extend inherited rules in order (negation is order-sensitive).
        // An explicit empty array clears them. Other arrays, such as enabled_languages,
        // replace their inherited value, including when the new array is empty.
        if let Some(patterns) = layer
            .get_mut("ignore")
            .and_then(|ignore| ignore.get_mut("patterns"))
            .and_then(toml::Value::as_array_mut)
            && !patterns.is_empty()
        {
            let mut inherited = config
                .get("ignore")
                .and_then(|ignore| ignore.get("patterns"))
                .and_then(toml::Value::as_array)
                .cloned()
                .unwrap_or_default();
            // save_project writes the effective rules, including their inherited prefix.
            // Reusing that prefix preserves ordering without duplicating it on every reload.
            if !patterns.starts_with(&inherited) {
                inherited.append(patterns);
                *patterns = inherited;
            }
        }

        fn overlay(base: &mut toml::Value, layer: toml::Value) {
            match (base, layer) {
                (toml::Value::Table(base), toml::Value::Table(layer)) => {
                    for (key, value) in layer {
                        if let Some(existing) = base.get_mut(&key) {
                            overlay(existing, value);
                        } else {
                            base.insert(key, value);
                        }
                    }
                }
                (base, value) => *base = value,
            }
        }

        overlay(config, layer);
    }
}
