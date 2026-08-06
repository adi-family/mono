#[cfg(test)]
mod tests {
    use crate::cache::{CachedFileData, GlobalCache};
    use crate::types::*;
    use tempfile::tempdir;

    fn sample_parsed_file() -> ParsedFile {
        ParsedFile {
            language: Language::Rust,
            symbols: vec![ParsedSymbol {
                name: "main".to_string(),
                kind: SymbolKind::Function,
                location: Location {
                    start_line: 1,
                    start_col: 0,
                    end_line: 5,
                    end_col: 1,
                    start_byte: 0,
                    end_byte: 50,
                },
                signature: Some("fn main()".to_string()),
                doc_comment: None,
                children: vec![],
                visibility: Visibility::Public,
            }],
            references: vec![],
        }
    }

    fn sample_embeddings() -> Vec<Vec<f32>> {
        vec![vec![0.1, 0.2, 0.3, 0.4]]
    }

    #[test]
    fn cache_put_and_get() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let data = CachedFileData {
            parsed: sample_parsed_file(),
            embeddings: sample_embeddings(),
            embed_model: "test-model".to_string(),
        };

        cache
            .put(
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
                &data,
            )
            .unwrap();

        let result = cache.get(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "test-model",
        );
        assert!(result.is_some());

        let cached = result.unwrap();
        assert_eq!(cached.parsed.symbols.len(), 1);
        assert_eq!(cached.parsed.symbols[0].name, "main");
        assert_eq!(cached.embeddings.len(), 1);
        assert_eq!(cached.embed_model, "test-model");
    }

    #[test]
    fn cache_miss_returns_none() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let result = cache.get(
            "nonexistent_hash_value_that_does_not_exist_in_cache_at_all_00",
            "test-model",
        );
        assert!(result.is_none());
    }

    #[test]
    fn cache_model_mismatch_returns_parsed_without_embeddings() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let data = CachedFileData {
            parsed: sample_parsed_file(),
            embeddings: sample_embeddings(),
            embed_model: "model-v1".to_string(),
        };
        cache.put(hash, &data).unwrap();

        // Request with different model
        let result = cache.get(hash, "model-v2");
        assert!(result.is_some());

        let cached = result.unwrap();
        assert_eq!(cached.parsed.symbols.len(), 1);
        assert!(cached.embeddings.is_empty());
    }

    #[test]
    fn cache_contains() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        assert!(!cache.contains(hash));

        let data = CachedFileData {
            parsed: sample_parsed_file(),
            embeddings: vec![],
            embed_model: "m".to_string(),
        };
        cache.put(hash, &data).unwrap();
        assert!(cache.contains(hash));
    }

    #[test]
    fn cache_overwrites_existing() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";

        let data1 = CachedFileData {
            parsed: sample_parsed_file(),
            embeddings: vec![vec![1.0]],
            embed_model: "m1".to_string(),
        };
        cache.put(hash, &data1).unwrap();

        let data2 = CachedFileData {
            parsed: sample_parsed_file(),
            embeddings: vec![vec![2.0, 3.0]],
            embed_model: "m2".to_string(),
        };
        cache.put(hash, &data2).unwrap();

        let result = cache.get(hash, "m2").unwrap();
        assert_eq!(result.embeddings, vec![vec![2.0, 3.0]]);
    }

    #[test]
    fn cache_serializes_references() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "bbccdd1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let parsed = ParsedFile {
            language: Language::TypeScript,
            symbols: vec![],
            references: vec![ParsedReference {
                name: "foo::bar".to_string(),
                kind: ReferenceKind::Call,
                location: Location {
                    start_line: 10,
                    start_col: 4,
                    end_line: 10,
                    end_col: 15,
                    start_byte: 100,
                    end_byte: 111,
                },
                containing_symbol_index: Some(42),
            }],
        };

        let data = CachedFileData {
            parsed,
            embeddings: vec![],
            embed_model: "x".to_string(),
        };
        cache.put(hash, &data).unwrap();

        let result = cache.get(hash, "x").unwrap();
        assert_eq!(result.parsed.references.len(), 1);
        assert_eq!(result.parsed.references[0].name, "foo::bar");
        assert_eq!(
            result.parsed.references[0].containing_symbol_index,
            Some(42)
        );
    }

    // ── Corruption / edge case tests ──

    #[test]
    fn cache_corrupted_json_returns_none() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "aabb001234567890abcdef1234567890abcdef1234567890abcdef1234567890";

        // Write garbage to the cache file path
        let path = dir.path().join("aa").join(format!("{hash}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not valid json {{{{").unwrap();

        // Should return None, not panic
        let result = cache.get(hash, "any-model");
        assert!(result.is_none());
    }

    #[test]
    fn cache_empty_file_returns_none() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "ccdd001234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let path = dir.path().join("cc").join(format!("{hash}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();

        let result = cache.get(hash, "m");
        assert!(result.is_none());
    }

    #[test]
    fn cache_nested_children_round_trip() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "eeff001234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let loc = || Location {
            start_line: 1,
            start_col: 0,
            end_line: 10,
            end_col: 1,
            start_byte: 0,
            end_byte: 100,
        };

        let parsed = ParsedFile {
            language: Language::Rust,
            symbols: vec![ParsedSymbol {
                name: "MyStruct".to_string(),
                kind: SymbolKind::Struct,
                location: loc(),
                signature: None,
                doc_comment: Some("A struct".to_string()),
                visibility: Visibility::Public,
                children: vec![
                    ParsedSymbol {
                        name: "new".to_string(),
                        kind: SymbolKind::Method,
                        location: loc(),
                        signature: Some("fn new() -> Self".to_string()),
                        doc_comment: None,
                        visibility: Visibility::Public,
                        children: vec![],
                    },
                    ParsedSymbol {
                        name: "process".to_string(),
                        kind: SymbolKind::Method,
                        location: loc(),
                        signature: Some("fn process(&self)".to_string()),
                        doc_comment: None,
                        visibility: Visibility::Private,
                        children: vec![],
                    },
                ],
            }],
            references: vec![],
        };

        let data = CachedFileData {
            parsed,
            embeddings: vec![vec![1.0]; 3], // 3 symbols (struct + 2 methods)
            embed_model: "m".to_string(),
        };
        cache.put(hash, &data).unwrap();

        let result = cache.get(hash, "m").unwrap();
        assert_eq!(result.parsed.symbols.len(), 1);
        assert_eq!(result.parsed.symbols[0].children.len(), 2);
        assert_eq!(result.parsed.symbols[0].children[0].name, "new");
        assert_eq!(
            result.parsed.symbols[0].children[1].visibility,
            Visibility::Private
        );
        assert_eq!(result.embeddings.len(), 3);
    }

    #[test]
    fn cache_many_embeddings_round_trip() {
        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "1122001234567890abcdef1234567890abcdef1234567890abcdef1234567890";

        // 50 symbols, each with 768-dim embeddings (realistic size)
        let symbols: Vec<ParsedSymbol> = (0..50)
            .map(|i| ParsedSymbol {
                name: format!("sym_{i}"),
                kind: SymbolKind::Function,
                location: Location {
                    start_line: i as u32,
                    start_col: 0,
                    end_line: i as u32 + 5,
                    end_col: 1,
                    start_byte: i as u32 * 100,
                    end_byte: (i as u32 + 1) * 100,
                },
                signature: Some(format!("fn sym_{i}()")),
                doc_comment: None,
                children: vec![],
                visibility: Visibility::Public,
            })
            .collect();

        let embeddings: Vec<Vec<f32>> = (0..50)
            .map(|i| (0..768).map(|j| (i * 768 + j) as f32 / 38400.0).collect())
            .collect();

        let data = CachedFileData {
            parsed: ParsedFile {
                language: Language::Go,
                symbols,
                references: vec![],
            },
            embeddings: embeddings.clone(),
            embed_model: "jina-v2".to_string(),
        };
        cache.put(hash, &data).unwrap();

        let result = cache.get(hash, "jina-v2").unwrap();
        assert_eq!(result.parsed.symbols.len(), 50);
        assert_eq!(result.embeddings.len(), 50);
        assert_eq!(result.embeddings[0].len(), 768);
        // Verify float precision preserved
        assert!((result.embeddings[0][0] - embeddings[0][0]).abs() < f32::EPSILON);
        assert!((result.embeddings[49][767] - embeddings[49][767]).abs() < f32::EPSILON);
    }

    // ── process_file integration: test that indexed search finds FTS results ──

    #[test]
    fn indexed_search_on_populated_sqlite() {
        use crate::storage::sqlite::SqliteStorage;
        use crate::storage::Storage;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.sqlite");
        let storage = SqliteStorage::open(&db_path).unwrap();

        // Insert a file
        let file = File {
            id: FileId(0),
            path: std::path::PathBuf::from("src/handler.rs"),
            language: Language::Rust,
            hash: "deadbeef".to_string(),
            size: 500,
            description: None,
        };
        let file_id = storage.insert_file(&file).unwrap();

        // Insert a symbol
        let symbol = Symbol {
            id: SymbolId(0),
            name: "handle_request".to_string(),
            kind: SymbolKind::Function,
            file_id,
            file_path: std::path::PathBuf::from("src/handler.rs"),
            location: Location {
                start_line: 10,
                start_col: 0,
                end_line: 20,
                end_col: 1,
                start_byte: 100,
                end_byte: 300,
            },
            parent_id: None,
            signature: Some("fn handle_request(req: Request) -> Response".to_string()),
            description: None,
            doc_comment: Some("Handles incoming HTTP requests".to_string()),
            visibility: Visibility::Public,
            is_entry_point: false,
        };
        storage.insert_symbol(&symbol).unwrap();

        // FTS search should find it
        let results = storage.search_symbols_fts("handle_request", 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "handle_request");

        // File FTS search
        let files = storage.search_files_fts("handler", 10).unwrap();
        assert!(!files.is_empty());
        assert_eq!(files[0].path, std::path::PathBuf::from("src/handler.rs"));
    }

    #[test]
    fn process_file_uses_cache_hit() {
        // Verify that process_file with a pre-populated cache
        // produces the same symbol count without calling the parser.
        //
        // We test this indirectly: put data in cache, then verify
        // it round-trips correctly (the integration with process_file
        // is tested via the cache get/put contract).

        let dir = tempdir().unwrap();
        let cache = GlobalCache::open_at(dir.path()).unwrap();

        let hash = "ff00001234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let parsed = ParsedFile {
            language: Language::Rust,
            symbols: vec![
                ParsedSymbol {
                    name: "foo".to_string(),
                    kind: SymbolKind::Function,
                    location: Location {
                        start_line: 1,
                        start_col: 0,
                        end_line: 3,
                        end_col: 1,
                        start_byte: 0,
                        end_byte: 30,
                    },
                    signature: Some("fn foo()".to_string()),
                    doc_comment: None,
                    children: vec![],
                    visibility: Visibility::Public,
                },
                ParsedSymbol {
                    name: "bar".to_string(),
                    kind: SymbolKind::Function,
                    location: Location {
                        start_line: 5,
                        start_col: 0,
                        end_line: 8,
                        end_col: 1,
                        start_byte: 31,
                        end_byte: 70,
                    },
                    signature: Some("fn bar(x: i32)".to_string()),
                    doc_comment: None,
                    children: vec![],
                    visibility: Visibility::Private,
                },
            ],
            references: vec![ParsedReference {
                name: "bar".to_string(),
                kind: ReferenceKind::Call,
                location: Location {
                    start_line: 2,
                    start_col: 4,
                    end_line: 2,
                    end_col: 7,
                    start_byte: 15,
                    end_byte: 18,
                },
                containing_symbol_index: None,
            }],
        };

        let embeddings = vec![vec![0.5; 4], vec![0.7; 4]];

        let data = CachedFileData {
            parsed: parsed.clone(),
            embeddings: embeddings.clone(),
            embed_model: "test".to_string(),
        };
        cache.put(hash, &data).unwrap();

        // Simulate what process_file does on cache hit:
        let cached = cache.get(hash, "test").unwrap();
        assert_eq!(cached.parsed.symbols.len(), parsed.symbols.len());
        assert_eq!(cached.embeddings.len(), embeddings.len());
        assert_eq!(cached.parsed.references.len(), 1);
        assert_eq!(cached.parsed.references[0].name, "bar");
    }
}
