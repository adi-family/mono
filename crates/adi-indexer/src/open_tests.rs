use super::*;

#[derive(Debug)]
struct SmallEmbedder;

impl Embedder for SmallEmbedder {
    fn embed(&self, texts: &[&str]) -> embed::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
    }

    fn dimensions(&self) -> u32 {
        4
    }

    fn model_name(&self) -> &'static str {
        "small-test-model"
    }
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::TempDir::with_prefix("indexer-open-").unwrap();
    std::fs::create_dir(dir.path().join(".adi")).unwrap();
    Config::default().save_project(dir.path()).unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn example() {}\n").unwrap();
    dir
}

#[test]
fn supplied_embedder_width_survives_indexing_and_reopening() {
    let dir = project();
    let mut config = Config::default();
    config.embedding.provider = "caller-supplied".into();
    config.embedding.model = "overridden-by-small-embedder".into();
    config.save_project(dir.path()).unwrap();
    let cache_dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let open = || {
        Indexer::open_with_cache(
            dir.path(),
            Arc::new(SmallEmbedder),
            GlobalCache::open_at(cache_dir.path()).unwrap(),
        )
        .unwrap()
    };

    let indexer = open();
    runtime.block_on(indexer.index()).unwrap();
    assert_eq!(indexer.index.count(), 1);
    assert_eq!(indexer.status().unwrap().embedding_dimensions, 4);
    drop(indexer);

    let indexer = open();
    let results = runtime.block_on(indexer.search("example", 1)).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].symbol.name, "example");
}

#[test]
fn no_embedder_still_opens_and_indexes_for_fts() {
    let dir = project();
    let cache_dir = tempfile::tempdir().unwrap();
    let indexer = Indexer::open_with_cache(
        dir.path(),
        Arc::new(embed::NoEmbedder),
        GlobalCache::open_at(cache_dir.path()).unwrap(),
    )
    .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(indexer.index()).unwrap();
    let results = runtime
        .block_on(indexer.search_symbols("example", 1))
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "example");
}

#[test]
fn unsupported_storage_and_zero_batch_size_fail_before_opening_project_storage() {
    for (backend, batch_size, expected) in [
        ("postgres", 32, "storage.backend"),
        ("sqlite", 0, "embedding.batch_size"),
    ] {
        let dir = project();
        let cache_dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.storage.backend = backend.into();
        config.embedding.batch_size = batch_size;
        config.save_project(dir.path()).unwrap();

        let error = Indexer::open_with_cache(
            dir.path(),
            Arc::new(SmallEmbedder),
            GlobalCache::open_at(cache_dir.path()).unwrap(),
        )
        .expect_err("invalid runtime config must be rejected");
        assert!(matches!(error, Error::Config(_)), "{error}");
        assert!(error.to_string().contains(expected), "{error}");
        assert!(!paths::index_dir(dir.path()).exists());
    }
}

#[test]
fn invalid_builtin_configuration_is_rejected_before_model_loading_or_index_creation() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for (field, value, expected) in [
        ("provider", "openai", "embedding.provider"),
        ("model", "unimplemented-model", "embedding.model"),
        ("dimensions", "16", "embedding.dimensions"),
        ("backend", "postgres", "storage.backend"),
        ("batch_size", "0", "embedding.batch_size"),
    ] {
        let dir = project();
        let mut config = Config::default();
        config.embedding.provider = "candle".into();
        match field {
            "provider" => config.embedding.provider = value.into(),
            "model" => config.embedding.model = value.into(),
            "dimensions" => config.embedding.dimensions = value.parse().unwrap(),
            "backend" => config.storage.backend = value.into(),
            "batch_size" => config.embedding.batch_size = value.parse().unwrap(),
            _ => unreachable!(),
        }
        config.save_project(dir.path()).unwrap();
        let error = runtime.block_on(Indexer::open(dir.path())).unwrap_err();
        assert!(matches!(error, Error::Config(_)), "{error}");
        assert!(error.to_string().contains(expected), "{error}");
        assert!(!paths::index_dir(dir.path()).exists());
    }
}

#[test]
fn explicit_none_provider_indexes_and_searches_without_a_model() {
    let dir = project();
    let cache_dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.embedding.provider = "none".into();
    let embedder = Indexer::configured_embedder(&config).unwrap();
    assert_eq!(embedder.dimensions(), 0);
    assert_eq!(embedder.model_name(), "none");
    let indexer = Indexer::open_configured(
        dir.path(),
        config,
        embedder,
        GlobalCache::open_at(cache_dir.path()).unwrap(),
    )
    .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(indexer.index()).unwrap();
    assert_eq!(indexer.index.count(), 0);
    let results = runtime
        .block_on(indexer.search_symbols("example", 1))
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "example");
    assert!(matches!(
        runtime.block_on(indexer.search("example", 1)),
        Err(Error::Embed(embed::EmbedError::Unavailable(_)))
    ));
}

#[cfg(not(feature = "candle"))]
#[test]
fn lean_build_defaults_to_none_but_rejects_an_explicit_candle_request() {
    let mut config = Config::default();
    assert_eq!(config.embedding.provider, "none");
    assert_eq!(
        Indexer::configured_embedder(&config).unwrap().model_name(),
        "none"
    );
    config.embedding.provider = "candle".into();
    let error = Indexer::configured_embedder(&config).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires a build with the `candle` feature"),
        "{error}"
    );
}

#[test]
fn no_embedder_rejects_a_zero_width_vector_index() {
    let mut config = Config::default();
    config.embedding.provider = "none".into();
    config.embedding.dimensions = 0;
    assert!(
        Indexer::configured_embedder(&config)
            .unwrap_err()
            .to_string()
            .contains("embedding.dimensions")
    );
}
