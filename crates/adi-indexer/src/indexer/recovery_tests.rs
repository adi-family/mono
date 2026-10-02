use super::{index_project, reindex_paths};
use crate::cache::GlobalCache;
use crate::config::Config;
use crate::embed::{EmbedError, Embedder};
use crate::parser::{Parser, TreeSitterParser};
use crate::search::{VectorIndex, usearch::UsearchIndex};
use crate::storage::{Storage, sqlite::SqliteStorage};
use std::sync::Arc;

#[derive(Debug)]
struct RecoveryEmbedder;

impl Embedder for RecoveryEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|_| vec![0.1, 0.2, 0.3, 0.4]).collect())
    }

    fn dimensions(&self) -> u32 {
        4
    }

    fn model_name(&self) -> &'static str {
        "recovery-test"
    }
}

#[test]
fn independently_opened_native_indexes_preserve_each_others_committed_vectors() {
    for path_limited in [false, true] {
        let project = tempfile::TempDir::with_prefix("indexer-recovery-project-").unwrap();
        let store = tempfile::tempdir().unwrap();
        let database = store.path().join("index.sqlite");
        let embeddings = store.path().join("embeddings");
        std::fs::create_dir_all(&embeddings).unwrap();

        // Both native handles initially see an empty index. A per-run writer lock alone
        // cannot keep the second handle from overwriting the first handle's later save.
        let first_storage: Arc<dyn Storage> = Arc::new(SqliteStorage::open(&database).unwrap());
        let second_storage: Arc<dyn Storage> = Arc::new(SqliteStorage::open(&database).unwrap());
        let first_index: Arc<dyn VectorIndex> =
            Arc::new(UsearchIndex::with_config(&embeddings, 4, 16, 200, 100).unwrap());
        let second_index: Arc<dyn VectorIndex> =
            Arc::new(UsearchIndex::with_config(&embeddings, 4, 16, 200, 100).unwrap());
        let embedder: Arc<dyn Embedder> = Arc::new(RecoveryEmbedder);
        let parser: Arc<dyn Parser> = Arc::new(TreeSitterParser::new());
        let cache = Arc::new(GlobalCache::open_at(&store.path().join("cache")).unwrap());
        let config = Config::default();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        std::fs::write(project.path().join("first.rs"), "fn first() {}\n").unwrap();
        let progress = runtime
            .block_on(index_project(
                project.path(),
                &config,
                first_storage.clone(),
                embedder.clone(),
                parser.clone(),
                first_index,
                cache.clone(),
            ))
            .unwrap();
        assert!(progress.errors.is_empty());
        let first = first_storage
            .find_symbols_by_name("first")
            .unwrap()
            .remove(0);

        std::fs::write(project.path().join("second.rs"), "fn second() {}\n").unwrap();
        if path_limited {
            runtime
                .block_on(reindex_paths(
                    project.path(),
                    &["second.rs".into()],
                    &config,
                    second_storage.clone(),
                    embedder,
                    parser,
                    second_index,
                    cache,
                ))
                .unwrap();
        } else {
            let progress = runtime
                .block_on(index_project(
                    project.path(),
                    &config,
                    second_storage.clone(),
                    embedder,
                    parser,
                    second_index,
                    cache,
                ))
                .unwrap();
            assert!(progress.errors.is_empty());
        }
        let second = second_storage
            .find_symbols_by_name("second")
            .unwrap()
            .remove(0);

        let reopened = UsearchIndex::with_config(&embeddings, 4, 16, 200, 100).unwrap();
        assert_eq!(
            reopened.count(),
            2,
            "stale handle lost a committed vector (path_limited={path_limited})"
        );
        assert!(reopened.get_vector(first.id.0).unwrap().is_some());
        assert!(reopened.get_vector(second.id.0).unwrap().is_some());
        assert!(second_storage.pending_vector_updates().unwrap().is_empty());
    }
}

#[test]
fn reopening_repairs_dirty_references_without_an_indexing_run() {
    for with_vectors in [false, true] {
        let project = tempfile::TempDir::with_prefix("indexer-recovery-project-").unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn Embedder> = if with_vectors {
            Arc::new(RecoveryEmbedder)
        } else {
            Arc::new(crate::embed::NoEmbedder)
        };
        let initial = crate::Indexer::open_with_cache(
            project.path(),
            embedder.clone(),
            GlobalCache::open_at(cache_dir.path()).unwrap(),
        )
        .unwrap();
        std::fs::write(
            project.path().join("lib.rs"),
            "fn target() {}\nfn caller() { target(); }\n",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let progress = runtime.block_on(initial.index()).unwrap();
        assert!(progress.errors.is_empty());
        let storage = initial.storage();
        let target = storage.find_symbols_by_name("target").unwrap().remove(0);
        let caller = storage.find_symbols_by_name("caller").unwrap().remove(0);
        assert_eq!(storage.get_callers(target.id).unwrap()[0].id, caller.id);

        // Simulate a committed checkpoint whose graph rebuild has not happened yet.
        // FTS-only indexing has no vector journal to use as an implicit dirty marker.
        storage.begin_transaction().unwrap();
        storage.replace_symbol_refs(&[]).unwrap();
        storage.set_references_dirty(true).unwrap();
        if with_vectors {
            storage
                .queue_vector_update(target.id, Some(&[0.1, 0.2, 0.3, 0.4]))
                .unwrap();
        }
        storage.commit_transaction().unwrap();
        let embeddings = crate::paths::index_dir(project.path()).join("embeddings");
        if with_vectors {
            let native = UsearchIndex::with_config(&embeddings, 4, 16, 200, 100).unwrap();
            native.remove(target.id.0).unwrap();
            native.save().unwrap();
        }
        drop(storage);
        drop(initial);

        let reopened = crate::Indexer::open_with_cache(
            project.path(),
            embedder,
            GlobalCache::open_at(cache_dir.path()).unwrap(),
        )
        .unwrap();
        let storage = reopened.storage();
        assert_eq!(storage.get_callers(target.id).unwrap()[0].id, caller.id);
        assert!(!storage.references_dirty().unwrap());
        assert!(storage.pending_vector_updates().unwrap().is_empty());
        if with_vectors {
            let native = UsearchIndex::with_config(&embeddings, 4, 16, 200, 100).unwrap();
            assert!(native.get_vector(target.id.0).unwrap().is_some());
            assert_eq!(native.count(), 2);
        }
    }
}
