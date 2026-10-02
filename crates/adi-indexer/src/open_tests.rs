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
