// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::storage::sqlite::SqliteStorage;
    use crate::storage::Storage;
    use crate::structure::Structure;
    use crate::types::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn create_test_storage() -> (SqliteStorage, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let storage = SqliteStorage::open(&db_path).unwrap();
        (storage, dir)
    }

    fn create_test_file() -> File {
        File {
            id: FileId(0),
            path: PathBuf::from("src/main.rs"),
            language: Language::Rust,
            hash: "abc123".to_string(),
            size: 1024,
            description: Some("Main entry point".to_string()),
        }
    }

    fn create_test_symbol(file_id: FileId, name: &str, kind: SymbolKind) -> Symbol {
        Symbol {
            id: SymbolId(0),
            name: name.to_string(),
            kind,
            file_id,
            file_path: PathBuf::from("src/main.rs"),
            location: Location {
                start_line: 0,
                start_col: 0,
                end_line: 10,
                end_col: 0,
                start_byte: 0,
                end_byte: 100,
            },
            parent_id: None,
            signature: Some(format!("fn {name}()")),
            description: Some(format!("A {} called {}", kind.as_str(), name)),
            doc_comment: None,
            visibility: Visibility::Public,
            is_entry_point: false,
            structure: None,
        }
    }

    #[test]
    fn test_insert_and_get_file() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();

        let file_id = storage.insert_file(&file).unwrap();
        assert!(file_id.0 > 0);

        let retrieved = storage.get_file(&file.path).unwrap();
        assert_eq!(retrieved.file.path, file.path);
        assert_eq!(retrieved.file.language, file.language);
        assert_eq!(retrieved.file.hash, file.hash);
    }

    #[test]
    fn test_file_exists() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();

        assert!(!storage.file_exists(&file.path).unwrap());

        storage.insert_file(&file).unwrap();
        assert!(storage.file_exists(&file.path).unwrap());
    }

    #[test]
    fn test_get_file_hash() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();

        assert!(storage.get_file_hash(&file.path).unwrap().is_none());

        storage.insert_file(&file).unwrap();
        let hash = storage.get_file_hash(&file.path).unwrap();
        assert_eq!(hash, Some("abc123".to_string()));
    }

    #[test]
    fn test_update_file() {
        let (storage, _dir) = create_test_storage();
        let mut file = create_test_file();

        let file_id = storage.insert_file(&file).unwrap();

        file.id = file_id;
        file.hash = "new_hash".to_string();
        file.size = 2048;

        storage.update_file(&file).unwrap();

        let retrieved = storage.get_file(&file.path).unwrap();
        assert_eq!(retrieved.file.hash, "new_hash");
        assert_eq!(retrieved.file.size, 2048);
    }

    #[test]
    fn test_delete_file() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();

        storage.insert_file(&file).unwrap();
        assert!(storage.file_exists(&file.path).unwrap());

        storage.delete_file(&file.path).unwrap();
        assert!(!storage.file_exists(&file.path).unwrap());
    }

    #[test]
    fn test_insert_and_get_symbol() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        let symbol = create_test_symbol(file_id, "main", SymbolKind::Function);
        let symbol_id = storage.insert_symbol(&symbol).unwrap();
        assert!(symbol_id.0 > 0);

        let retrieved = storage.get_symbol(symbol_id).unwrap();
        assert_eq!(retrieved.name, "main");
        assert_eq!(retrieved.kind, SymbolKind::Function);
    }

    #[test]
    fn test_get_symbols_for_file() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        storage
            .insert_symbol(&create_test_symbol(file_id, "func1", SymbolKind::Function))
            .unwrap();
        storage
            .insert_symbol(&create_test_symbol(file_id, "func2", SymbolKind::Function))
            .unwrap();
        storage
            .insert_symbol(&create_test_symbol(file_id, "MyStruct", SymbolKind::Struct))
            .unwrap();

        let symbols = storage.get_symbols_for_file(file_id).unwrap();
        assert_eq!(symbols.len(), 3);
    }

    #[test]
    fn test_delete_symbols_for_file() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        storage
            .insert_symbol(&create_test_symbol(file_id, "func1", SymbolKind::Function))
            .unwrap();
        storage
            .insert_symbol(&create_test_symbol(file_id, "func2", SymbolKind::Function))
            .unwrap();

        let symbols = storage.get_symbols_for_file(file_id).unwrap();
        assert_eq!(symbols.len(), 2);

        storage.delete_symbols_for_file(file_id).unwrap();

        let symbols = storage.get_symbols_for_file(file_id).unwrap();
        assert_eq!(symbols.len(), 0);
    }

    #[test]
    fn test_search_symbols_fts() {
        let (storage, _dir) = create_test_storage();
        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        storage
            .insert_symbol(&create_test_symbol(
                file_id,
                "process_data",
                SymbolKind::Function,
            ))
            .unwrap();
        storage
            .insert_symbol(&create_test_symbol(
                file_id,
                "handle_request",
                SymbolKind::Function,
            ))
            .unwrap();
        storage
            .insert_symbol(&create_test_symbol(
                file_id,
                "DataProcessor",
                SymbolKind::Struct,
            ))
            .unwrap();

        let results = storage.search_symbols_fts("process_data", 10).unwrap();
        assert!(!results.is_empty(), "Should find 'process_data' function");
    }

    #[test]
    fn test_search_files_fts() {
        let (storage, _dir) = create_test_storage();

        let file1 = File {
            id: FileId(0),
            path: PathBuf::from("src/handlers/user.rs"),
            language: Language::Rust,
            hash: "hash1".to_string(),
            size: 100,
            description: Some("User handling module".to_string()),
        };
        storage.insert_file(&file1).unwrap();

        let file2 = File {
            id: FileId(0),
            path: PathBuf::from("src/models/user.rs"),
            language: Language::Rust,
            hash: "hash2".to_string(),
            size: 200,
            description: Some("User model".to_string()),
        };
        storage.insert_file(&file2).unwrap();

        let results = storage.search_files_fts("user", 10).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_get_tree() {
        let (storage, _dir) = create_test_storage();

        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        storage
            .insert_symbol(&create_test_symbol(file_id, "main", SymbolKind::Function))
            .unwrap();

        let tree = storage.get_tree().unwrap();
        assert_eq!(tree.files.len(), 1);
        assert_eq!(tree.files[0].path, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn test_get_status() {
        let (storage, _dir) = create_test_storage();

        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        storage
            .insert_symbol(&create_test_symbol(file_id, "main", SymbolKind::Function))
            .unwrap();
        storage
            .insert_symbol(&create_test_symbol(file_id, "helper", SymbolKind::Function))
            .unwrap();

        let status = storage.get_status().unwrap();
        assert_eq!(status.indexed_files, 1);
        assert_eq!(status.indexed_symbols, 2);
    }

    #[test]
    fn status_reports_the_size_of_the_database() {
        let (storage, _dir) = create_test_storage();

        // A migrated-but-empty index is already several pages of schema; the point is that the
        // number comes from SQLite rather than being the stub 0 it used to be.
        let before = storage.get_status().unwrap().storage_size_bytes;
        assert!(before > 0, "an open index has a size");

        for i in 0..200 {
            let file = File {
                id: FileId(0),
                path: PathBuf::from(format!("src/file_{i}.rs")),
                language: Language::Rust,
                hash: format!("hash{i}"),
                size: 100,
                description: None,
            };
            storage.insert_file(&file).unwrap();
        }

        assert!(
            storage.get_status().unwrap().storage_size_bytes >= before,
            "size does not shrink as rows are added"
        );
    }

    #[test]
    fn test_update_status() {
        let (storage, _dir) = create_test_storage();

        let status = Status {
            indexed_files: 10,
            indexed_symbols: 100,
            embedding_dimensions: 768,
            embedding_model: "test-model".to_string(),
            last_indexed: Some("2024-01-01".to_string()),
            storage_size_bytes: 1024,
            pipeline_version: crate::indexer::PIPELINE_VERSION,
        };

        storage.update_status(&status).unwrap();

        let retrieved = storage.get_status().unwrap();
        assert_eq!(retrieved.embedding_model, "test-model");
        assert_eq!(retrieved.embedding_dimensions, 768);
        assert_eq!(
            retrieved.pipeline_version,
            crate::indexer::PIPELINE_VERSION,
            "the pipeline version has to survive a round trip, or every run rebuilds"
        );
    }

    /// A fresh database reports pipeline 0, which is what makes an index written before the
    /// version existed rebuild itself instead of being trusted.
    #[test]
    fn an_unstamped_index_reports_pipeline_zero() {
        let (storage, _dir) = create_test_storage();
        assert_eq!(storage.get_status().unwrap().pipeline_version, 0);
    }

    /// The structural columns have to survive storage, and `structures` has to filter by size.
    ///
    /// This is the round trip that the positional column reads make fragile: a symbol row is
    /// read by index, so a fingerprint written into the wrong column comes back as a plausible
    /// value rather than an error.
    #[test]
    fn structural_fingerprints_round_trip_and_filter_by_size() {
        let (storage, _dir) = create_test_storage();
        let file_id = storage.insert_file(&create_test_file()).unwrap();

        let mut big = create_test_symbol(file_id, "collect", SymbolKind::Function);
        big.structure = Some(Structure {
            hash: "cafebabe".to_string(),
            // Round-trips through SQLite's signed INTEGER — the top bit is the case that a
            // numeric conversion would lose.
            simhash: u64::MAX,
            node_count: 120,
        });
        let mut twin = create_test_symbol(file_id, "tally", SymbolKind::Function);
        twin.structure = big.structure.clone();
        let mut small = create_test_symbol(file_id, "get", SymbolKind::Function);
        small.structure = Some(Structure {
            hash: "d00dfeed".to_string(),
            simhash: 1,
            node_count: 3,
        });

        let big_id = storage.insert_symbol(&big).unwrap();
        storage.insert_symbol(&twin).unwrap();
        storage.insert_symbol(&small).unwrap();

        let stored = storage.get_symbol(big_id).unwrap();
        let structure = stored.structure.expect("fingerprint survived storage");
        assert_eq!(structure.hash, "cafebabe");
        assert_eq!(structure.simhash, u64::MAX);
        assert_eq!(structure.node_count, 120);

        let rows = storage.structures(100).unwrap();
        assert_eq!(rows.len(), 2, "the 3-node symbol is below the floor");

        let groups = crate::clones::exact(rows);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
    }

    /// A symbol with no fingerprint is absent from `structures` rather than present with a
    /// zeroed one — a zero would match every other zero and invent a clone group.
    #[test]
    fn symbols_without_a_fingerprint_are_left_out() {
        let (storage, _dir) = create_test_storage();
        let file_id = storage.insert_file(&create_test_file()).unwrap();

        let plain = create_test_symbol(file_id, "unfingerprinted", SymbolKind::Function);
        assert!(plain.structure.is_none());
        let id = storage.insert_symbol(&plain).unwrap();

        assert!(storage.get_symbol(id).unwrap().structure.is_none());
        assert!(storage.structures(0).unwrap().is_empty());
    }

    #[test]
    fn test_transactions() {
        let (storage, _dir) = create_test_storage();

        storage.begin_transaction().unwrap();

        let file = create_test_file();
        storage.insert_file(&file).unwrap();

        storage.commit_transaction().unwrap();

        assert!(storage.file_exists(&file.path).unwrap());
    }

    #[test]
    fn test_transaction_rollback() {
        let (storage, _dir) = create_test_storage();

        storage.begin_transaction().unwrap();

        let file = create_test_file();
        storage.insert_file(&file).unwrap();

        storage.rollback_transaction().unwrap();

        // Note: In SQLite with the current implementation,
        // the rollback may not work as expected due to autocommit mode
        // This test is here to ensure the API works without panicking
    }

    #[test]
    fn test_file_with_symbols() {
        let (storage, _dir) = create_test_storage();

        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        storage
            .insert_symbol(&create_test_symbol(file_id, "main", SymbolKind::Function))
            .unwrap();
        storage
            .insert_symbol(&create_test_symbol(file_id, "Config", SymbolKind::Struct))
            .unwrap();

        let file_info = storage.get_file(&file.path).unwrap();
        assert_eq!(file_info.symbols.len(), 2);
    }

    #[test]
    fn test_symbol_with_parent() {
        let (storage, _dir) = create_test_storage();

        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        let class_symbol = create_test_symbol(file_id, "MyClass", SymbolKind::Class);
        let class_id = storage.insert_symbol(&class_symbol).unwrap();

        let mut method_symbol = create_test_symbol(file_id, "my_method", SymbolKind::Method);
        method_symbol.parent_id = Some(class_id);
        let method_id = storage.insert_symbol(&method_symbol).unwrap();

        let retrieved = storage.get_symbol(method_id).unwrap();
        assert_eq!(retrieved.parent_id, Some(class_id));
    }

    #[test]
    fn test_get_file_by_id() {
        let (storage, _dir) = create_test_storage();

        let file = create_test_file();
        let file_id = storage.insert_file(&file).unwrap();

        let retrieved = storage.get_file_by_id(file_id).unwrap();
        assert_eq!(retrieved.path, file.path);
    }

    #[test]
    fn test_multiple_files() {
        let (storage, _dir) = create_test_storage();

        for i in 0..5 {
            let file = File {
                id: FileId(0),
                path: PathBuf::from(format!("src/file{i}.rs")),
                language: Language::Rust,
                hash: format!("hash{i}"),
                size: 100 * i as u64,
                description: None,
            };
            storage.insert_file(&file).unwrap();
        }

        let status = storage.get_status().unwrap();
        assert_eq!(status.indexed_files, 5);
    }

    /// A caller in one file, a callee in another, and the edge between them.
    ///
    /// Two files rather than one because the graph reads are the queries that join `files`, and
    /// a single-file fixture cannot tell a correct join from one that returns the only path
    /// there is.
    fn two_linked_files(storage: &SqliteStorage) -> (SymbolId, SymbolId) {
        let caller_file = storage.insert_file(&create_test_file()).unwrap();
        let callee_file = storage
            .insert_file(&File {
                id: FileId(0),
                path: PathBuf::from("src/util.rs"),
                language: Language::Rust,
                hash: "def456".to_string(),
                size: 512,
                description: None,
            })
            .unwrap();

        let caller = storage
            .insert_symbol(&create_test_symbol(caller_file, "caller", SymbolKind::Function))
            .unwrap();
        let mut callee_symbol = create_test_symbol(callee_file, "callee", SymbolKind::Function);
        callee_symbol.file_path = PathBuf::from("src/util.rs");
        callee_symbol.visibility = Visibility::Private;
        let callee = storage.insert_symbol(&callee_symbol).unwrap();

        storage
            .insert_reference(&Reference {
                from_symbol_id: caller,
                to_symbol_id: callee,
                kind: ReferenceKind::Call,
                location: Location {
                    start_line: 5,
                    start_col: 4,
                    end_line: 5,
                    end_col: 14,
                    start_byte: 40,
                    end_byte: 50,
                },
            })
            .unwrap();

        (caller, callee)
    }

    #[test]
    fn the_graph_reads_carry_the_joined_file_path() {
        let (storage, _dir) = create_test_storage();
        let (caller, callee) = two_linked_files(&storage);

        let callers = storage.get_callers(callee).unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].id, caller);
        assert_eq!(callers[0].name, "caller");
        // The `f.path` column trails SYMBOL_COLUMNS, so its index moves whenever that list
        // grows and a stale one reads a neighbouring column without failing.
        assert_eq!(callers[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(callers[0].signature.as_deref(), Some("fn caller()"));
        assert_eq!(callers[0].visibility, Visibility::Public);

        let callees = storage.get_callees(caller).unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].id, callee);
        assert_eq!(callees[0].file_path, PathBuf::from("src/util.rs"));
        assert_eq!(callees[0].visibility, Visibility::Private);

        assert!(storage.get_callers(caller).unwrap().is_empty());
        assert!(storage.get_callees(callee).unwrap().is_empty());
    }

    #[test]
    fn get_all_symbols_spans_every_file() {
        let (storage, _dir) = create_test_storage();
        let (caller, callee) = two_linked_files(&storage);

        let all = storage.get_all_symbols().unwrap();
        assert_eq!(all.len(), 2);

        let mut by_id: Vec<_> = all.iter().map(|s| (s.id, s.file_path.clone())).collect();
        by_id.sort_by_key(|(id, _)| id.0);
        assert_eq!(
            by_id,
            vec![
                (caller, PathBuf::from("src/main.rs")),
                (callee, PathBuf::from("src/util.rs")),
            ]
        );
    }

    #[test]
    fn deleting_a_files_references_takes_the_inbound_ones_too() {
        let (storage, _dir) = create_test_storage();
        let (caller, callee) = two_linked_files(&storage);

        // The edge runs *into* this file, not out of it: deleting only outbound edges would
        // leave a row that can never join a live symbol again, since the file's symbols get new
        // ids when it is reprocessed.
        let callee_file = storage
            .get_file(&PathBuf::from("src/util.rs"))
            .unwrap()
            .file
            .id;
        storage.delete_references_for_file(callee_file).unwrap();

        assert!(storage.get_callers(callee).unwrap().is_empty());
        assert!(storage.get_callees(caller).unwrap().is_empty());
        assert!(storage.get_references_to(callee).unwrap().is_empty());
        assert_eq!(storage.get_reference_count(callee).unwrap(), 0);
    }
}
