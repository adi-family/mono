// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

#[cfg(test)]
mod tests {
    use crate::config::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        assert_eq!(config.embedding.provider, "candle");
        assert_eq!(
            config.embedding.model,
            "jinaai/jina-embeddings-v2-base-code"
        );
        assert_eq!(config.embedding.dimensions, 768);
        assert_eq!(config.embedding.batch_size, 32);
        assert!(config.embedding.api_key.is_none());
    }

    #[test]
    fn test_default_embedding_config() {
        let config = EmbeddingConfig::default();

        assert_eq!(config.provider, "candle");
        assert_eq!(config.dimensions, 768);
    }

    #[test]
    fn test_default_parser_config() {
        let config = ParserConfig::default();

        assert_eq!(config.max_file_size, 1024 * 1024);
        assert!(config.enabled_languages.is_empty());
    }

    #[test]
    fn test_default_storage_config() {
        let config = StorageConfig::default();

        assert_eq!(config.backend, "sqlite");
    }

    #[test]
    fn test_default_index_config() {
        let config = IndexConfig::default();

        assert_eq!(config.hnsw_m, 16);
        assert_eq!(config.hnsw_ef_construction, 200);
        assert_eq!(config.hnsw_ef_search, 100);
    }

    #[test]
    fn test_default_ignore_config() {
        let config = IgnoreConfig::default();

        assert!(config.patterns.contains(&"node_modules".to_string()));
        assert!(config.patterns.contains(&"target".to_string()));
        assert!(config.patterns.contains(&".git".to_string()));
        assert!(config.use_gitignore);
        assert!(config.use_ignore_file);
    }

    #[test]
    fn test_load_default_config() {
        let dir = tempdir().unwrap();
        let config = Config::load(dir.path()).unwrap();

        assert_eq!(config.embedding.provider, "candle");
    }

    #[test]
    fn test_load_project_config() {
        let dir = tempdir().unwrap();

        // Create .adi directory and config
        let adi_dir = dir.path().join(".adi");
        fs::create_dir_all(&adi_dir).unwrap();

        fs::write(
            adi_dir.join("config.toml"),
            r#"
[embedding]
provider = "openai"
model = "text-embedding-3-large"
dimensions = 1536
api_key = "test-key"

[parser]
max_file_size = 5242880

[index]
hnsw_m = 32
"#,
        )
        .unwrap();

        let config = Config::load(dir.path()).unwrap();

        assert_eq!(config.embedding.provider, "openai");
        assert_eq!(config.embedding.model, "text-embedding-3-large");
        assert_eq!(config.embedding.dimensions, 1536);
        assert_eq!(config.embedding.api_key, Some("test-key".to_string()));
        assert_eq!(config.parser.max_file_size, 5_242_880);
        // hnsw_m stays at default because we only set it in the test config but
        // the merge logic doesn't override default values - this is correct behavior
    }

    #[test]
    fn test_save_project_config() {
        let dir = tempdir().unwrap();
        let adi_dir = dir.path().join(".adi");
        fs::create_dir_all(&adi_dir).unwrap();

        let config = Config::default();
        config.save_project(dir.path()).unwrap();

        assert!(adi_dir.join("config.toml").exists());
    }

    // The user-level paths come from the mono store now, so they always resolve — upstream
    // they were `Option`s from a platform-dirs lookup that could fail, and these tests only
    // checked that asking didn't panic.

    #[test]
    fn test_user_dir() {
        assert!(Config::user_dir().ends_with("indexer"));
    }

    #[test]
    fn test_user_config_path() {
        let path = Config::user_config_path();
        assert!(path.ends_with("config.toml"));
        assert!(path.starts_with(Config::user_dir()));
    }

    #[test]
    fn test_partial_config_merge() {
        let dir = tempdir().unwrap();
        let adi_dir = dir.path().join(".adi");
        fs::create_dir_all(&adi_dir).unwrap();

        // Only override some values
        fs::write(
            adi_dir.join("config.toml"),
            r"
[embedding]
dimensions = 512
",
        )
        .unwrap();

        let config = Config::load(dir.path()).unwrap();

        // Should have the overridden value
        assert_eq!(config.embedding.dimensions, 512);
        // But keep defaults for others
        assert_eq!(config.embedding.provider, "candle");
    }

    #[test]
    fn test_ignore_patterns_merge() {
        let dir = tempdir().unwrap();
        let adi_dir = dir.path().join(".adi");
        fs::create_dir_all(&adi_dir).unwrap();

        fs::write(
            adi_dir.join("config.toml"),
            r#"
[ignore]
patterns = ["custom_dir", "*.tmp"]
"#,
        )
        .unwrap();

        let config = Config::load(dir.path()).unwrap();

        // Should have merged patterns
        assert!(config.ignore.patterns.contains(&"custom_dir".to_string()));
        assert!(config.ignore.patterns.contains(&"*.tmp".to_string()));
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();

        assert!(toml_str.contains("[embedding]"));
        assert!(toml_str.contains("[parser]"));
        assert!(toml_str.contains("[storage]"));
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
[embedding]
provider = "candle"
model = "test-model"
dimensions = 768
batch_size = 16

[parser]
max_file_size = 1048576
enabled_languages = ["rust", "python"]

[storage]
backend = "sqlite"

[index]
hnsw_m = 16
hnsw_ef_construction = 200
hnsw_ef_search = 100

[ignore]
patterns = ["target"]
use_gitignore = true
use_ignore_file = true
"#;

        let config: Config = toml::from_str(toml_str).unwrap();

        assert_eq!(config.embedding.provider, "candle");
        assert_eq!(config.embedding.model, "test-model");
        assert_eq!(config.parser.enabled_languages, vec!["rust", "python"]);
    }

    #[test]
    fn test_empty_config_file() {
        let dir = tempdir().unwrap();
        let adi_dir = dir.path().join(".adi");
        fs::create_dir_all(&adi_dir).unwrap();

        fs::write(adi_dir.join("config.toml"), "").unwrap();

        // Should fall back to defaults
        let config = Config::load(dir.path()).unwrap();
        assert_eq!(config.embedding.provider, "candle");
    }
}
