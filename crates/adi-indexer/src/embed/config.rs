// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Embedding configuration.

use crate::embed::error::Result;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Configuration for embedding providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    /// Provider name (e.g., "fastembed")
    pub provider: String,
    /// Model identifier
    pub model: String,
    /// Embedding dimensions
    pub dimensions: u32,
    /// Batch size for embedding
    pub batch_size: usize,
    /// Optional API key for remote providers
    pub api_key: Option<String>,
    /// Optional API base URL for remote providers
    pub api_base: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "candle".to_string(),
            model: "jinaai/jina-embeddings-v2-base-code".to_string(),
            dimensions: 768,
            batch_size: 32,
            api_key: None,
            api_base: None,
        }
    }
}

impl EmbeddingConfig {
    /// Load embedding config from user config file if it exists.
    pub fn load_user_config() -> Result<Self> {
        let config_path = paths::user_config_path();
        if config_path.exists() {
            return Self::load_from_file(&config_path);
        }
        Ok(Self::default())
    }

    /// Load config from a TOML file.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let wrapper: ConfigWrapper = toml::from_str(&content)?;
        Ok(wrapper.embedding)
    }

    /// Merge with another config, preferring non-default values from other.
    #[must_use]
    pub fn merge(mut self, other: Self) -> Self {
        let default = Self::default();

        if other.provider != default.provider {
            self.provider = other.provider;
        }
        if other.model != default.model {
            self.model = other.model;
        }
        if other.dimensions != default.dimensions {
            self.dimensions = other.dimensions;
        }
        if other.batch_size != default.batch_size {
            self.batch_size = other.batch_size;
        }
        if other.api_key.is_some() {
            self.api_key = other.api_key;
        }
        if other.api_base.is_some() {
            self.api_base = other.api_base;
        }

        self
    }
}

/// Wrapper for TOML config files that may contain embedding section.
#[derive(Debug, Deserialize)]
struct ConfigWrapper {
    #[serde(default)]
    embedding: EmbeddingConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.provider, "candle");
        assert_eq!(config.dimensions, 768);
    }

    #[test]
    fn test_merge_prefers_non_default() {
        let base = EmbeddingConfig::default();
        let other = EmbeddingConfig {
            provider: "custom".to_string(),
            model: "custom-model".to_string(),
            dimensions: 1024,
            ..Default::default()
        };

        let merged = base.merge(other);
        assert_eq!(merged.provider, "custom");
        assert_eq!(merged.model, "custom-model");
        assert_eq!(merged.dimensions, 1024);
    }
}
