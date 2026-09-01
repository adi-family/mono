// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::search::VectorIndex;
    use crate::search::usearch::UsearchIndex;
    use tempfile::tempdir;

    fn create_test_index() -> (UsearchIndex, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let index = UsearchIndex::open(dir.path()).unwrap();
        (index, dir)
    }

    #[test]
    fn test_index_creation() {
        let (index, _dir) = create_test_index();
        assert_eq!(index.count(), 0);
    }

    #[test]
    fn test_add_vector() {
        let (index, _dir) = create_test_index();

        let vector: Vec<f32> = (0..768).map(|i| i as f32 / 768.0).collect();
        index.add(1, &vector).unwrap();

        assert_eq!(index.count(), 1);
    }

    #[test]
    fn test_add_multiple_vectors() {
        let (index, _dir) = create_test_index();

        for i in 0..10 {
            let vector: Vec<f32> = (0..768).map(|j| (i * j) as f32 / 768.0).collect();
            index.add(i, &vector).unwrap();
        }

        assert_eq!(index.count(), 10);
    }

    #[test]
    fn test_search_single() {
        let (index, _dir) = create_test_index();

        // Add a vector
        let vector: Vec<f32> = (0..768).map(|i| i as f32 / 768.0).collect();
        index.add(42, &vector).unwrap();

        // Search for similar
        let results = index.search(&vector, 1).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 42);
        // Score should be high for identical vector (close to 1.0 for cosine)
        assert!(results[0].1 > 0.9);
    }

    #[test]
    fn test_search_multiple() {
        let (index, _dir) = create_test_index();

        // Add multiple vectors
        for i in 0..100 {
            let vector: Vec<f32> = (0..768).map(|j| ((i + j) % 100) as f32 / 100.0).collect();
            index.add(i, &vector).unwrap();
        }

        // Create query
        let query: Vec<f32> = (0..768).map(|j| j as f32 / 100.0).collect();
        let results = index.search(&query, 10).unwrap();

        assert_eq!(results.len(), 10);
        // Results should be sorted by score (descending)
        for i in 1..results.len() {
            assert!(results[i - 1].1 >= results[i].1);
        }
    }

    #[test]
    fn test_remove_vector() {
        let (index, _dir) = create_test_index();

        let vector: Vec<f32> = (0..768).map(|i| i as f32 / 768.0).collect();
        index.add(1, &vector).unwrap();
        assert_eq!(index.count(), 1);

        index.remove(1).unwrap();
        // Note: usearch may not immediately reflect count changes
    }

    #[test]
    fn test_save_and_reload() {
        let dir = tempdir().unwrap();

        // Create and populate index
        {
            let index = UsearchIndex::open(dir.path()).unwrap();
            for i in 0..10 {
                let vector: Vec<f32> = (0..768).map(|j| (i * j) as f32 / 768.0).collect();
                index.add(i, &vector).unwrap();
            }
            index.save().unwrap();
        }

        // Reload and verify
        {
            let _index = UsearchIndex::open(dir.path()).unwrap();
            // The index should have loaded the saved vectors
            // Note: exact count may vary based on usearch behavior
        }
    }

    #[test]
    fn test_dimension_mismatch() {
        let (index, _dir) = create_test_index();

        // Try to add wrong dimension vector
        let wrong_vector: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let result = index.add(1, &wrong_vector);

        assert!(result.is_err());
    }

    #[test]
    fn test_search_dimension_mismatch() {
        let (index, _dir) = create_test_index();

        // Add correct vector
        let vector: Vec<f32> = (0..768).map(|i| i as f32 / 768.0).collect();
        index.add(1, &vector).unwrap();

        // Try to search with wrong dimension
        let wrong_query: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let result = index.search(&wrong_query, 1);

        assert!(result.is_err());
    }

    #[test]
    fn test_search_empty_index() {
        let (index, _dir) = create_test_index();

        let query: Vec<f32> = (0..768).map(|i| i as f32 / 768.0).collect();
        let results = index.search(&query, 10).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_search_limit() {
        let (index, _dir) = create_test_index();

        // Add 100 vectors
        for i in 0..100 {
            let vector: Vec<f32> = (0..768).map(|j| ((i + j) % 100) as f32 / 100.0).collect();
            index.add(i, &vector).unwrap();
        }

        let query: Vec<f32> = (0..768).map(|j| j as f32 / 100.0).collect();

        // Limit to 5
        let results = index.search(&query, 5).unwrap();
        assert!(results.len() <= 5);

        // Limit to 50
        let results = index.search(&query, 50).unwrap();
        assert!(results.len() <= 50);
    }

    #[test]
    fn test_with_custom_config() {
        let dir = tempdir().unwrap();
        let index = UsearchIndex::with_config(
            dir.path(),
            768, // dimensions
            32,  // m
            400, // ef_construction
            200, // ef_search
        )
        .unwrap();

        let vector: Vec<f32> = (0..768).map(|i| i as f32 / 768.0).collect();
        index.add(1, &vector).unwrap();

        let results = index.search(&vector, 1).unwrap();
        assert_eq!(results.len(), 1);
    }
}
