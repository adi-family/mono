// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

#[cfg(test)]
mod tests {
    use crate::storage::mmap::EmbeddingStore;
    use tempfile::tempdir;

    #[test]
    fn test_create_store() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 768, model_hash).unwrap();
        assert_eq!(store.dimensions(), 768);
    }

    #[test]
    fn test_open_existing_store() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        // Create
        {
            let _store = EmbeddingStore::create(&path, 768, model_hash).unwrap();
        }

        // Open
        {
            let store = EmbeddingStore::open(&path).unwrap();
            assert_eq!(store.dimensions(), 768);
        }
    }

    #[test]
    fn test_open_or_create_new() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::open_or_create(&path, 768, model_hash).unwrap();
        assert_eq!(store.dimensions(), 768);
    }

    #[test]
    fn test_open_or_create_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        // Create first
        {
            let _store = EmbeddingStore::create(&path, 768, model_hash).unwrap();
        }

        // Open or create should open existing
        let store = EmbeddingStore::open_or_create(&path, 768, model_hash).unwrap();
        assert_eq!(store.dimensions(), 768);
    }

    #[test]
    fn test_append_and_get() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 4, model_hash).unwrap();

        // Append some embeddings
        let embeddings = vec![vec![1.0, 2.0, 3.0, 4.0], vec![5.0, 6.0, 7.0, 8.0]];
        store.append(&embeddings).unwrap();

        // Get them back
        let first = store.get(0).unwrap();
        assert_eq!(first, vec![1.0, 2.0, 3.0, 4.0]);

        let second = store.get(1).unwrap();
        assert_eq!(second, vec![5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn test_count() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 4, model_hash).unwrap();
        assert_eq!(store.count().unwrap(), 0);

        store.append(&[vec![1.0, 2.0, 3.0, 4.0]]).unwrap();
        assert_eq!(store.count().unwrap(), 1);

        store
            .append(&[vec![5.0, 6.0, 7.0, 8.0], vec![9.0, 10.0, 11.0, 12.0]])
            .unwrap();
        assert_eq!(store.count().unwrap(), 3);
    }

    #[test]
    fn test_get_batch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 4, model_hash).unwrap();

        let embeddings = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![5.0, 6.0, 7.0, 8.0],
            vec![9.0, 10.0, 11.0, 12.0],
        ];
        store.append(&embeddings).unwrap();

        let batch = store.get_batch(&[0, 2]).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0], vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(batch[1], vec![9.0, 10.0, 11.0, 12.0]);
    }

    #[test]
    fn test_iter() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 4, model_hash).unwrap();

        let embeddings = vec![vec![1.0, 2.0, 3.0, 4.0], vec![5.0, 6.0, 7.0, 8.0]];
        store.append(&embeddings).unwrap();

        let iter_results: Vec<_> = store.iter().unwrap().collect();
        assert_eq!(iter_results.len(), 2);
        assert_eq!(iter_results[0], vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(iter_results[1], vec![5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn test_dimension_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 4, model_hash).unwrap();

        // Try to append wrong dimension
        let wrong_embeddings = vec![vec![1.0, 2.0, 3.0]]; // 3 instead of 4
        let result = store.append(&wrong_embeddings);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_out_of_bounds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 4, model_hash).unwrap();
        store.append(&[vec![1.0, 2.0, 3.0, 4.0]]).unwrap();

        let result = store.get(999);
        assert!(result.is_err());
    }

    #[test]
    fn test_append_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 4, model_hash).unwrap();
        store.append(&[]).unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn test_large_embeddings() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embeddings.bin");
        let model_hash = [0u8; 32];

        let store = EmbeddingStore::create(&path, 768, model_hash).unwrap();

        // Add 100 embeddings of 768 dimensions
        for i in 0..100 {
            let embedding: Vec<f32> = (0..768).map(|j| (i * j) as f32 / 768.0).collect();
            store.append(&[embedding]).unwrap();
        }

        assert_eq!(store.count().unwrap(), 100);

        // Verify a few
        let first = store.get(0).unwrap();
        assert_eq!(first.len(), 768);

        let last = store.get(99).unwrap();
        assert_eq!(last.len(), 768);
    }
}
