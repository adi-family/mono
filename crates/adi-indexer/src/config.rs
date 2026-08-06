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
        let mut config = Self::default();

        // User-level defaults from the indexer's module in the mono store.
        let user_config_path = Self::user_config_path();
        if user_config_path.exists() {
            let content = std::fs::read_to_string(&user_config_path)?;
            let user_config: Config = toml::from_str(&content)?;
            config = config.merge(user_config);
        }

        // Load project-level config from .adi/config.toml
        let project_config_path = project_path.join(".adi/config.toml");
        if project_config_path.exists() {
            let content = std::fs::read_to_string(&project_config_path)?;
            let project_config: Config = toml::from_str(&content)?;
            config = config.merge(project_config);
        }

        Ok(config)
    }

    pub fn save_project(&self, project_path: &Path) -> Result<()> {
        let config_path = project_path.join(".adi/config.toml");
        let content = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
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

    fn merge(mut self, other: Config) -> Self {
        // Override with non-default values from other
        if other.embedding.provider != EmbeddingConfig::default().provider {
            self.embedding.provider = other.embedding.provider;
        }
        if other.embedding.model != EmbeddingConfig::default().model {
            self.embedding.model = other.embedding.model;
        }
        if other.embedding.dimensions != EmbeddingConfig::default().dimensions {
            self.embedding.dimensions = other.embedding.dimensions;
        }
        if other.embedding.api_key.is_some() {
            self.embedding.api_key = other.embedding.api_key;
        }
        if other.embedding.api_base.is_some() {
            self.embedding.api_base = other.embedding.api_base;
        }
        if other.parser.max_file_size != ParserConfig::default().max_file_size {
            self.parser.max_file_size = other.parser.max_file_size;
        }
        if !other.parser.enabled_languages.is_empty() {
            self.parser.enabled_languages = other.parser.enabled_languages;
        }
        if !other.ignore.patterns.is_empty()
            && other.ignore.patterns != IgnoreConfig::default().patterns
        {
            self.ignore.patterns.extend(other.ignore.patterns);
        }
        self
    }
}
