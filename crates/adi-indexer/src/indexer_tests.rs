// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

#[cfg(test)]
mod tests {
    use crate::cache::GlobalCache;
    use crate::config::Config;
    use crate::embed::{EmbedError, Embedder};
    use crate::error::Result;
    use crate::indexer::index_project;
    use crate::parser::{Parser, TreeSitterParser};
    use crate::search::VectorIndex;
    use crate::storage::Storage;
    use crate::storage::sqlite::SqliteStorage;
    use crate::types::{IndexProgress, Symbol};
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// The same vector for every text: these tests are about what the pipeline does with an
    /// embedding, never about what is in one.
    #[derive(Debug)]
    struct StubEmbedder;

    impl Embedder for StubEmbedder {
        fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
            Ok(texts.iter().map(|_| vec![0.5; 4]).collect())
        }

        fn dimensions(&self) -> u32 {
            4
        }

        fn model_name(&self) -> &'static str {
            "stub"
        }
    }

    /// A vector index that is just the map it was told to hold, so a test can ask whether a
    /// pruned symbol's embedding went with its rows.
    #[derive(Debug, Default)]
    struct RecordingIndex {
        vectors: Mutex<HashMap<i64, Vec<f32>>>,
    }

    impl VectorIndex for RecordingIndex {
        fn add(&self, id: i64, vector: &[f32]) -> Result<()> {
            self.vectors.lock().unwrap().insert(id, vector.to_vec());
            Ok(())
        }

        fn remove(&self, id: i64) -> Result<()> {
            self.vectors.lock().unwrap().remove(&id);
            Ok(())
        }

        fn search(&self, _query: &[f32], _limit: usize) -> Result<Vec<(i64, f32)>> {
            Ok(vec![])
        }

        fn get_vector(&self, id: i64) -> Result<Option<Vec<f32>>> {
            Ok(self.vectors.lock().unwrap().get(&id).cloned())
        }

        fn save(&self) -> Result<()> {
            Ok(())
        }

        fn count(&self) -> usize {
            self.vectors.lock().unwrap().len()
        }
    }

    /// A project directory and an index over it, both temporary.
    ///
    /// The index deliberately does *not* live in the project's own `.adi/tree`: keeping it out
    /// of the walk means a test asserts about the files it wrote and nothing else.
    struct Fixture {
        project: TempDir,
        /// Dropping this directory would delete the index out from under `storage`.
        store: TempDir,
        storage: Arc<dyn Storage>,
        index: Arc<RecordingIndex>,
        embedder: Arc<dyn Embedder>,
        parser: Arc<dyn Parser>,
        cache: Arc<GlobalCache>,
    }

    impl Fixture {
        fn new() -> Self {
            // Not `tempdir()`: its default prefix is `.tmp`, and a hidden root is a walk that
            // finds nothing.
            let project = TempDir::with_prefix("indexer-project-").unwrap();
            let store = TempDir::with_prefix("indexer-store-").unwrap();

            let storage = SqliteStorage::open(&store.path().join("index.sqlite")).unwrap();
            let cache = GlobalCache::open_at(&store.path().join("cache")).unwrap();

            Self {
                project,
                store,
                storage: Arc::new(storage),
                index: Arc::new(RecordingIndex::default()),
                embedder: Arc::new(StubEmbedder),
                parser: Arc::new(TreeSitterParser::new()),
                cache: Arc::new(cache),
            }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.project.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }

        fn delete(&self, relative: &str) {
            std::fs::remove_file(self.project.path().join(relative)).unwrap();
        }

        fn run(&self, config: &Config) -> IndexProgress {
            self.run_with_embedder(config, self.embedder.clone())
        }

        /// A run with a specific embedder, for the tests where the point is what happens when
        /// the one the fixture started with is not the one this run gets.
        fn run_with_embedder(&self, config: &Config, embedder: Arc<dyn Embedder>) -> IndexProgress {
            let index: Arc<dyn VectorIndex> = self.index.clone();

            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(index_project(
                    self.project.path(),
                    config,
                    self.storage.clone(),
                    embedder,
                    self.parser.clone(),
                    index,
                    self.cache.clone(),
                ))
                .unwrap()
        }

        fn symbol(&self, name: &str) -> Option<Symbol> {
            self.storage
                .find_symbols_by_name(name)
                .unwrap()
                .into_iter()
                .next()
        }

        fn indexed(&self, relative: &str) -> bool {
            self.storage.file_exists(Path::new(relative)).unwrap()
        }

        fn reindex(&self, paths: &[&str], config: &Config) -> Result<()> {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(crate::indexer::reindex_paths(
                    self.project.path(),
                    &paths
                        .iter()
                        .map(std::path::PathBuf::from)
                        .collect::<Vec<_>>(),
                    config,
                    self.storage.clone(),
                    self.embedder.clone(),
                    self.parser.clone(),
                    self.index.clone(),
                    self.cache.clone(),
                ))
        }
    }

    #[test]
    fn ignore_patterns_match_path_components_and_globs_not_substrings() {
        let fixture = Fixture::new();
        for path in [
            "build.rs",
            "src/targeting.rs",
            "src/distribution.rs",
            "target/out.rs",
            "nested/build/out.rs",
            "src/generated_api.rs",
            "src/generated_keep.rs",
        ] {
            fixture.write(path, "fn example() {}\n");
        }
        let mut config = Config::default();
        config
            .ignore
            .patterns
            .extend(["**/generated_*.rs".into(), "!src/generated_keep.rs".into()]);
        fixture.run(&config);
        for path in [
            "build.rs",
            "src/targeting.rs",
            "src/distribution.rs",
            "src/generated_keep.rs",
        ] {
            assert!(fixture.indexed(path), "unexpectedly ignored {path}");
        }
        for path in [
            "target/out.rs",
            "nested/build/out.rs",
            "src/generated_api.rs",
        ] {
            assert!(!fixture.indexed(path), "failed to ignore {path}");
        }
    }

    #[test]
    fn disabling_a_language_prunes_its_previous_index() {
        let fixture = Fixture::new();
        fixture.write("src/lib.rs", "fn present() {}\n");
        fixture.run(&Config::default());
        assert!(fixture.indexed("src/lib.rs"));
        let mut config = Config::default();
        config.parser.enabled_languages = vec!["python".into()];
        let progress = fixture.run(&config);
        assert_eq!(progress.files_total, 0);
        assert!(!fixture.indexed("src/lib.rs"));
    }

    #[test]
    fn unsupported_files_are_not_counted_as_indexed_work() {
        let fixture = Fixture::new();
        fixture.write("data.json", "{}");
        let progress = fixture.run(&Config::default());
        assert_eq!(progress.files_total, 0);
        assert_eq!(progress.files_processed, 0);
    }

    #[test]
    fn watch_updates_preserve_inbound_edges_and_replace_outbound_edges() {
        let fixture = Fixture::new();
        fixture.write("caller.rs", "fn caller() { changed(); }\n");
        fixture.write("changed.rs", "fn changed() { previous(); }\n");
        fixture.write("targets.rs", "fn previous() {}\nfn next() {}\n");
        let config = Config::default();
        fixture.run(&config);
        fixture.write("changed.rs", "fn changed() { next(); }\n");
        fixture
            .reindex(&["changed.rs", "changed.rs"], &config)
            .unwrap();
        let changed = fixture.symbol("changed").unwrap();
        let caller = fixture.symbol("caller").unwrap();
        let next = fixture.symbol("next").unwrap();
        assert_eq!(
            fixture.storage.get_callers(changed.id).unwrap()[0].id,
            caller.id
        );
        assert_eq!(
            fixture.storage.get_callees(changed.id).unwrap()[0].id,
            next.id
        );
        assert_eq!(fixture.storage.all_pending_refs().unwrap().len(), 2);
        fixture.run(&config);
        assert_eq!(
            fixture.storage.get_callees(changed.id).unwrap()[0].id,
            next.id
        );
    }

    #[test]
    fn watch_deleting_a_directory_removes_descendants_and_vectors() {
        let fixture = Fixture::new();
        fixture.write("nested/one.rs", "fn one() {}\n");
        fixture.write("nested/two.rs", "fn two() {}\n");
        fixture.write("keep.rs", "fn keep() {}\n");
        let config = Config::default();
        fixture.run(&config);
        std::fs::remove_dir_all(fixture.project.path().join("nested")).unwrap();
        fixture.reindex(&["nested"], &config).unwrap();
        assert!(!fixture.indexed("nested/one.rs"));
        assert!(!fixture.indexed("nested/two.rs"));
        assert!(fixture.indexed("keep.rs"));
        assert_eq!(fixture.index.count(), 1);
    }

    #[test]
    fn watch_updates_honor_size_and_ignore_rules() {
        let fixture = Fixture::new();
        fixture.write("small.rs", "fn small() {}\n");
        let mut config = Config::default();
        fixture.run(&config);
        fixture.write("target/new.rs", "fn ignored() {}\n");
        config.parser.max_file_size = 5;
        fixture
            .reindex(&["small.rs", "target/new.rs"], &config)
            .unwrap();
        assert!(!fixture.indexed("small.rs"));
        assert!(!fixture.indexed("target/new.rs"));
        assert_eq!(fixture.index.count(), 0);
    }

    #[test]
    fn changing_an_ignore_file_updates_eligibility_for_the_whole_tree() {
        let fixture = Fixture::new();
        fixture.write("keep.rs", "fn keep() {}\n");
        fixture.write("gone.rs", "fn gone() {}\n");
        let config = Config::default();
        fixture.run(&config);
        fixture.write(".ignore", "gone.rs\n");
        fixture.reindex(&[".ignore"], &config).unwrap();
        assert!(!fixture.indexed("gone.rs"));
        assert!(fixture.indexed("keep.rs"));
        fixture.delete(".ignore");
        fixture.reindex(&[".ignore"], &config).unwrap();
        assert!(fixture.indexed("gone.rs"));
    }

    #[test]
    fn an_interrupted_pipeline_upgrade_is_retried_for_unchanged_files() {
        #[derive(Debug)]
        struct FailingParser;
        impl Parser for FailingParser {
            fn supports(&self, _: crate::types::Language) -> bool {
                true
            }
            fn parse(
                &self,
                _: &str,
                _: crate::types::Language,
            ) -> Result<crate::types::ParsedFile> {
                Err(crate::error::Error::Parser("temporary failure".into()))
            }
        }
        let mut fixture = Fixture::new();
        fixture.write("lib.rs", "fn original() {}\n");
        let config = Config::default();
        fixture.run(&config);
        let original = fixture.symbol("original").unwrap();
        let mut old_status = fixture.storage.get_status().unwrap();
        old_status.pipeline_version -= 1;
        fixture.storage.update_status(&old_status).unwrap();
        fixture.cache = Arc::new(
            GlobalCache::open_at(&fixture.store.path().join("cache_after_upgrade")).unwrap(),
        );
        fixture.parser = Arc::new(FailingParser);
        assert_eq!(fixture.run(&config).errors.len(), 1);
        fixture.parser = Arc::new(TreeSitterParser::new());
        assert!(fixture.run(&config).errors.is_empty());
        assert_ne!(
            fixture.symbol("original").unwrap().id,
            original.id,
            "the failed pipeline upgrade was marked complete and never retried"
        );
        assert_eq!(
            fixture.storage.get_status().unwrap().pipeline_version,
            crate::indexer::PIPELINE_VERSION
        );
    }

    #[test]
    fn a_watch_read_error_preserves_the_last_good_file_and_is_reported() {
        let fixture = Fixture::new();
        fixture.write("valid.rs", "fn valid() {}\n");
        let config = Config::default();
        fixture.run(&config);
        let before = fixture.symbol("valid").unwrap();
        std::fs::write(fixture.project.path().join("valid.rs"), [0xff]).unwrap();
        assert!(fixture.reindex(&["valid.rs"], &config).is_err());
        assert_eq!(fixture.symbol("valid").unwrap().id, before.id);
        fixture.write("valid.rs", "fn recovered() {}\n");
        fixture.reindex(&["valid.rs"], &config).unwrap();
        assert!(fixture.symbol("recovered").is_some());
    }

    #[test]
    fn cached_parses_do_not_cross_language_boundaries() {
        #[derive(Debug)]
        struct LanguageParser;
        impl Parser for LanguageParser {
            fn supports(&self, _: crate::types::Language) -> bool {
                true
            }
            fn parse(
                &self,
                _: &str,
                language: crate::types::Language,
            ) -> Result<crate::types::ParsedFile> {
                Ok(crate::types::ParsedFile {
                    language,
                    symbols: vec![crate::types::ParsedSymbol::new(
                        language.as_str(),
                        crate::types::SymbolKind::Function,
                        crate::types::Location {
                            start_line: 0,
                            start_col: 0,
                            end_line: 0,
                            end_col: 1,
                            start_byte: 0,
                            end_byte: 1,
                        },
                    )],
                    references: vec![],
                })
            }
        }
        let mut fixture = Fixture::new();
        fixture.parser = Arc::new(LanguageParser);
        fixture.write("one.rs", "identical bytes");
        fixture.run(&Config::default());
        fixture.write("two.py", "identical bytes");
        fixture.run(&Config::default());
        assert!(fixture.symbol("rust").is_some());
        assert!(
            fixture.symbol("python").is_some(),
            "a Python file reused the Rust parse"
        );
    }

    /// A run walks the files that are there, so a deleted one is never looked at and no
    /// comparison of content hashes can notice it left.
    #[test]
    fn a_deleted_file_takes_its_rows_with_it() {
        let fixture = Fixture::new();
        fixture.write(
            "src/keep.rs",
            "pub fn kept_alive() {\n    departed_helper();\n}\n",
        );
        fixture.write(
            "src/gone.rs",
            "pub fn departed_helper() {\n    let total = 1 + 2;\n    println!(\"{total}\");\n}\n",
        );

        let config = Config::default();
        fixture.run(&config);

        let departed = fixture.symbol("departed_helper").expect("indexed once");
        assert!(
            fixture.index.get_vector(departed.id.0).unwrap().is_some(),
            "the symbol should have been embedded before it was deleted"
        );
        assert!(
            !fixture
                .storage
                .get_references_to(departed.id)
                .unwrap()
                .is_empty(),
            "the call in keep.rs should have resolved to it"
        );

        fixture.delete("src/gone.rs");
        fixture.run(&config);

        assert!(!fixture.indexed("src/gone.rs"), "the file row survived");
        assert!(
            fixture
                .storage
                .find_symbols_by_name("departed_helper")
                .unwrap()
                .is_empty(),
            "the symbol rows survived"
        );
        assert!(
            fixture
                .storage
                .search_symbols_fts("departed_helper", 10)
                .unwrap()
                .is_empty(),
            "symbols_fts still answers for the symbol"
        );
        assert!(
            fixture
                .storage
                .search_files_fts("gone", 10)
                .unwrap()
                .is_empty(),
            "files_fts still answers for the file"
        );
        assert!(
            fixture
                .storage
                .get_references_to(departed.id)
                .unwrap()
                .is_empty(),
            "references pointing at the deleted symbol survived"
        );
        assert!(
            fixture.index.get_vector(departed.id.0).unwrap().is_none(),
            "the embedding outlived the symbol it stood for"
        );

        assert!(fixture.indexed("src/keep.rs"), "a live file was pruned");
        assert!(
            fixture.symbol("kept_alive").is_some(),
            "a live symbol was pruned"
        );
    }

    /// Ignore rules are what "in scope" means, so a file the config now excludes has to leave
    /// the index as surely as one deleted from disk.
    #[test]
    fn a_newly_ignored_file_is_pruned_too() {
        let fixture = Fixture::new();
        fixture.write("src/keep.rs", "pub fn kept_alive() {}\n");
        fixture.write("generated/api.rs", "pub fn generated_symbol() {}\n");

        fixture.run(&Config::default());
        assert!(fixture.indexed("generated/api.rs"));

        let mut config = Config::default();
        config.ignore.patterns.push("generated".to_string());
        fixture.run(&config);

        assert!(!fixture.indexed("generated/api.rs"));
        assert!(fixture.symbol("generated_symbol").is_none());
        assert!(fixture.indexed("src/keep.rs"));
    }

    /// Pruning is not a rebuild: a run that finds everything where it left it must delete
    /// nothing, however many files it skipped as unchanged.
    #[test]
    fn an_unchanged_tree_loses_nothing() {
        let fixture = Fixture::new();
        fixture.write("src/keep.rs", "pub fn kept_alive() {}\n");
        fixture.write("src/other.rs", "pub fn also_kept() {}\n");

        let config = Config::default();
        fixture.run(&config);
        let before = fixture.storage.get_status().unwrap();
        let embedded = fixture.index.count();

        fixture.run(&config);

        let after = fixture.storage.get_status().unwrap();
        assert_eq!(before.indexed_files, after.indexed_files);
        assert_eq!(before.indexed_symbols, after.indexed_symbols);
        assert_eq!(embedded, fixture.index.count());
    }

    /// A model swap must not leave an untouched file's vectors from the *old* model sitting in
    /// the index forever, silently mixed in with whatever a later run does re-embed —
    /// `docs/embedding-backends-survey.md`'s Surprise #1.
    #[test]
    fn a_model_swap_reembeds_a_file_even_when_nothing_else_about_it_changed() {
        #[derive(Debug)]
        struct SecondEmbedder;
        impl Embedder for SecondEmbedder {
            fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
                Ok(texts.iter().map(|_| vec![0.9; 4]).collect())
            }
            fn dimensions(&self) -> u32 {
                4
            }
            fn model_name(&self) -> &'static str {
                "second"
            }
        }

        let fixture = Fixture::new();
        fixture.write("src/keep.rs", "pub fn kept_alive() {}\n");
        let config = Config::default();
        fixture.run(&config);

        let before = fixture.symbol("kept_alive").expect("indexed");
        assert_eq!(
            fixture.index.get_vector(before.id.0).unwrap().unwrap(),
            vec![0.5; 4],
            "the stub's own vector"
        );

        fixture.run_with_embedder(&config, Arc::new(SecondEmbedder));

        assert_eq!(
            fixture.storage.get_status().unwrap().embedding_model,
            "second"
        );
        let after = fixture.symbol("kept_alive").expect("still indexed");
        assert_eq!(
            fixture.index.get_vector(after.id.0).unwrap().unwrap(),
            vec![0.9; 4],
            "an untouched file must still be re-embedded when the model changes"
        );
    }

    /// Reprocessing a file gives its symbols new ids, and every edge pointing into it dies with
    /// the old ones — including edges from files this run had no reason to reparse.
    #[test]
    fn an_edge_from_an_unchanged_file_survives_its_target_being_rewritten() {
        let fixture = Fixture::new();
        fixture.write(
            "src/caller.rs",
            "pub fn calls_out() {\n    the_callee();\n}\n",
        );
        fixture.write("src/callee.rs", "pub fn the_callee() {}\n");

        let config = Config::default();
        fixture.run(&config);

        let target = fixture.symbol("the_callee").expect("indexed");
        assert_eq!(
            fixture.storage.get_references_to(target.id).unwrap().len(),
            1,
            "the call should resolve on the first run"
        );

        // Only the target changes. caller.rs is byte-identical, so the next run skips it and never
        // sees the call again — the edge has to be rebuilt from what the index already holds.
        fixture.write(
            "src/callee.rs",
            "pub fn the_callee() {\n    let _ = 1 + 1;\n}\n",
        );
        fixture.run(&config);

        let target = fixture.symbol("the_callee").expect("still indexed");
        assert_eq!(
            fixture.storage.get_references_to(target.id).unwrap().len(),
            1,
            "the caller did not change, so the edge into the target should still be there"
        );
    }

    /// A directory the walk could not read looks exactly like a directory whose files were all
    /// deleted, and the difference is the whole index.
    #[cfg(unix)]
    #[test]
    fn a_directory_the_walk_could_not_read_keeps_its_rows() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new();
        fixture.write("src/keep.rs", "pub fn kept_alive() {}\n");
        fixture.write("locked/hidden.rs", "pub fn still_here() {}\n");

        let config = Config::default();
        fixture.run(&config);
        assert!(fixture.indexed("locked/hidden.rs"));

        let locked = fixture.project.path().join("locked");
        let readable = std::fs::metadata(&locked).unwrap().permissions();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Running as root, the mode is advisory and the walk reads the directory anyway; there
        // is no failed walk to assert about then.
        let unreadable = std::fs::read_dir(&locked).is_err();
        if unreadable {
            fixture.run(&config);
        }
        std::fs::set_permissions(&locked, readable).unwrap();

        if unreadable {
            assert!(
                fixture.indexed("locked/hidden.rs"),
                "a file under an unreadable directory was pruned as though it had left the tree"
            );
            assert!(fixture.indexed("src/keep.rs"));
        }
    }
}
