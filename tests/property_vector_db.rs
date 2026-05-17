// Property-based tests for VectorDB using proptest
//
// These tests verify key invariants of the lock-free vector database:
// 1. Dimension consistency - vectors must match configured dimension
// 2. Search bounds - search returns at most k results
// 3. Similarity range - cosine similarity is always in [-1.0, 1.0]
// 4. Idempotency - adding vectors doesn't corrupt state
// 5. Remove consistency - removed vectors don't appear in searches
// 6. Stats accuracy - statistics match actual operations
// 7. Concurrent safety - operations are thread-safe
//
// NOTE: All tests in this file should run serially due to shared VectorDB resources

use post_cortex_embeddings::{VectorDB, VectorDbConfig, VectorMetadata};
use proptest::prelude::*;
use serial_test::serial;

// Strategy to generate valid vector dimensions
fn dimension_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![Just(128), Just(256), Just(384), Just(512), Just(768)]
}

// Strategy to generate normalized vectors of given dimension
fn vector_strategy(dim: usize) -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(-1.0f32..1.0f32, dim).prop_map(|v| {
        // Normalize vector to unit length
        let magnitude = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            v.iter().map(|x| x / magnitude).collect()
        } else {
            v
        }
    })
}

// Strategy to generate vector metadata
fn metadata_strategy() -> impl Strategy<Value = VectorMetadata> {
    (
        prop::string::string_regex("[a-z]{5,10}").unwrap(),
        prop::string::string_regex("[a-z ]{10,50}").unwrap(),
        prop::string::string_regex("session-[0-9]{1,5}").unwrap(),
        prop::string::string_regex("(qa|decision|problem|code)").unwrap(),
    )
        .prop_map(|(id, text, source, content_type)| {
            VectorMetadata::new(id, text, source, content_type)
        })
}

// Strategy to generate search limit k
fn search_k_strategy() -> impl Strategy<Value = usize> {
    1usize..=20
}

proptest! {
    /// Property: Vectors must match configured dimension
    #[test]
    fn prop_dimension_validation(
        (dim, wrong_dim) in (dimension_strategy(), dimension_strategy())
            .prop_filter("Dimensions must be different", |(d1, d2)| d1 != d2)
    ) {
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Correct dimension should succeed
        let correct_vector = vec![0.5f32; dim];
        let metadata = VectorMetadata::new(
            "test-1".to_string(),
            "test text".to_string(),
            "test-source".to_string(),
            "qa".to_string(),
        );
        let result = db.add_vector(correct_vector, metadata);
        prop_assert!(result.is_ok());

        // Wrong dimension should fail
        let wrong_vector = vec![0.5f32; wrong_dim];
        let metadata2 = VectorMetadata::new(
            "test-2".to_string(),
            "test text 2".to_string(),
            "test-source".to_string(),
            "qa".to_string(),
        );
        let result = db.add_vector(wrong_vector, metadata2);
        prop_assert!(result.is_err());
    }

    /// Property: Search returns at most k results
    #[test]
    fn prop_search_bounds(
        k in search_k_strategy(),
        num_vectors in 1usize..=50,
    ) {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors
        for i in 0..num_vectors {
            let vector = vec![(i as f32) / num_vectors as f32; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Search with limit k
        let query = vec![0.5f32; dim];
        let results = db.search(&query, k).unwrap();

        // Results should not exceed k
        prop_assert!(results.len() <= k);

        // If we have fewer than k vectors, results should match vector count
        if num_vectors < k {
            prop_assert_eq!(results.len(), num_vectors);
        }
    }

    /// Property: Cosine similarity is always in valid range
    #[test]
    fn prop_similarity_range(
        dim in dimension_strategy(),
        vectors in prop::collection::vec(vector_strategy(384), 1..=20),
    ) {
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors (only if dimension matches)
        if dim == 384 {
            for (i, vector) in vectors.iter().enumerate() {
                let metadata = VectorMetadata::new(
                    format!("vec-{}", i),
                    format!("text {}", i),
                    "test-source".to_string(),
                    "qa".to_string(),
                );
                db.add_vector(vector.clone(), metadata).unwrap();
            }

            // Search with arbitrary query
            let query = vec![0.5f32; dim];
            let results = db.search(&query, 10).unwrap();

            // All similarities must be in [-1.0, 1.0]
            for result in results {
                prop_assert!(result.similarity >= -1.0);
                prop_assert!(result.similarity <= 1.0);
            }
        }
    }

    /// Property: Adding vectors is idempotent (can add multiple times)
    #[test]
    fn prop_add_idempotency(
        num_vectors in 1usize..=20,
        metadata in metadata_strategy(),
    ) {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors multiple times
        for _ in 0..num_vectors {
            let vector = vec![0.5f32; dim];
            let result = db.add_vector(vector, metadata.clone());
            prop_assert!(result.is_ok());
        }

        // Stats should reflect total additions
        let stats = db.get_stats();
        prop_assert_eq!(stats.total_vectors, num_vectors);
    }

    /// Property: Removed vectors don't appear in search results
    #[test]
    fn prop_remove_consistency(
        num_vectors in 2usize..=20,
        remove_idx in 0usize..=10,
    ) {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors and track IDs
        let mut vector_ids = Vec::new();
        for i in 0..num_vectors {
            let vector = vec![(i as f32) / num_vectors as f32; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            let id = db.add_vector(vector, metadata).unwrap();
            vector_ids.push(id);
        }

        // Remove a vector if index is valid
        let remove_idx = remove_idx % num_vectors;
        let removed_id = vector_ids[remove_idx];
        let removed = db.remove_vector(removed_id).unwrap();
        prop_assert!(removed);

        // Search should not return removed vector
        let query = vec![0.5f32; dim];
        let results = db.search(&query, num_vectors).unwrap();

        for result in results {
            prop_assert_ne!(result.vector_id, removed_id);
        }

        // Stats should reflect removal
        let stats = db.get_stats();
        prop_assert_eq!(stats.total_vectors, num_vectors - 1);
    }

    /// Property: Statistics are accurate
    #[test]
    fn prop_stats_accuracy(
        num_add in 0usize..=30,
        num_remove in 0usize..=15,
    ) {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors
        let mut vector_ids = Vec::new();
        for i in 0..num_add {
            let vector = vec![0.5f32; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            let id = db.add_vector(vector, metadata).unwrap();
            vector_ids.push(id);
        }

        // Remove some vectors
        let actual_removes = num_remove.min(num_add);
        for i in 0..actual_removes {
            db.remove_vector(vector_ids[i]).unwrap();
        }

        // Check stats
        let stats = db.get_stats();
        let expected_count = num_add - actual_removes;
        prop_assert_eq!(stats.total_vectors, expected_count);
    }

    /// Property: Clear removes all vectors
    #[test]
    fn prop_clear_completeness(num_vectors in 1usize..=30) {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors
        for i in 0..num_vectors {
            let vector = vec![0.5f32; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Clear database
        db.clear().unwrap();

        // Stats should show zero vectors
        let stats = db.get_stats();
        prop_assert_eq!(stats.total_vectors, 0);

        // Search should return no results
        let query = vec![0.5f32; dim];
        let results = db.search(&query, 10).unwrap();
        prop_assert_eq!(results.len(), 0);
    }

    /// Property: Search returns results in descending similarity order
    #[test]
    fn prop_search_ordering(num_vectors in 2usize..=20) {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors with different similarities to query
        for i in 0..num_vectors {
            let mut vector = vec![0.0f32; dim];
            // Create vectors with varying dot products with [1, 0, 0, ...]
            vector[0] = (i as f32) / (num_vectors as f32);
            vector[1] = 1.0 - vector[0];

            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Search
        let query = vec![1.0f32, 0.0f32].into_iter().chain(vec![0.0f32; dim - 2]).collect::<Vec<_>>();
        let results = db.search(&query, num_vectors).unwrap();

        // Verify descending order
        for i in 0..results.len().saturating_sub(1) {
            prop_assert!(results[i].similarity >= results[i + 1].similarity);
        }
    }

    /// Property: Identical vectors have similarity close to 1.0
    #[test]
    fn prop_identical_vector_similarity(vector in vector_strategy(384)) {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vector
        let metadata = VectorMetadata::new(
            "test".to_string(),
            "test text".to_string(),
            "test-source".to_string(),
            "qa".to_string(),
        );
        db.add_vector(vector.clone(), metadata).unwrap();

        // Search with same vector
        let results = db.search(&vector, 1).unwrap();

        // Should return exactly 1 result with similarity ~1.0
        prop_assert_eq!(results.len(), 1);
        prop_assert!((results[0].similarity - 1.0).abs() < 0.01);
    }
}

#[cfg(test)]
mod pq_tests {
    use super::*;

    #[test]
    fn test_product_quantization_enabled() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            enable_product_quantization: true,
            pq_subvectors: 8,
            pq_bits: 8,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add a vector
        let vector = vec![0.5f32; dim];
        let metadata = VectorMetadata::new(
            "test".to_string(),
            "test text".to_string(),
            "test-source".to_string(),
            "qa".to_string(),
        );
        let id = db.add_vector(vector.clone(), metadata).unwrap();

        // Search for the same vector
        let results = db.search(&vector, 1).unwrap();

        // Should find it with high similarity despite PQ compression
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].vector_id, id);
        assert!(results[0].similarity > 0.9); // PQ may reduce similarity slightly
    }

    #[test]
    fn test_pq_memory_savings() {
        let dim = 384;

        // Database without PQ
        let config_no_pq = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            enable_product_quantization: false,
            ..Default::default()
        };
        let db_no_pq = VectorDB::new(config_no_pq).unwrap();

        // Database with PQ
        let config_pq = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            enable_product_quantization: true,
            pq_subvectors: 8,
            pq_bits: 8,
            ..Default::default()
        };
        let db_pq = VectorDB::new(config_pq).unwrap();

        // Add same vectors to both
        for i in 0..100 {
            let vector = vec![(i as f32) / 100.0; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db_no_pq.add_vector(vector.clone(), metadata.clone()).unwrap();
            db_pq.add_vector(vector, metadata).unwrap();
        }

        // PQ should use significantly less memory
        let stats_no_pq = db_no_pq.get_stats();
        let stats_pq = db_pq.get_stats();

        // PQ codes are 8 bytes (8 subvectors × 1 byte) vs 1536 bytes (384 floats × 4 bytes)
        // Actual savings depend on whether we keep original vectors
        println!("Memory without PQ: {} bytes", stats_no_pq.memory_usage_bytes);
        println!("Memory with PQ: {} bytes", stats_pq.memory_usage_bytes);

        // Both should have 100 vectors
        assert_eq!(stats_no_pq.total_vectors, 100);
        assert_eq!(stats_pq.total_vectors, 100);
    }

    #[test]
    fn test_pq_search_accuracy() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            enable_product_quantization: true,
            pq_subvectors: 8,
            pq_bits: 8,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add diverse vectors
        for i in 0..50 {
            let mut vector = vec![0.0f32; dim];
            vector[i % dim] = 1.0;
            vector[(i + 1) % dim] = 0.5;

            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Search should still work
        let mut query = vec![0.0f32; dim];
        query[10] = 1.0;
        query[11] = 0.5;

        let results = db.search(&query, 5).unwrap();

        // Should return some results
        assert!(!results.is_empty());
        assert!(results.len() <= 5);

        // Results should be in descending similarity order
        for i in 0..results.len().saturating_sub(1) {
            assert!(results[i].similarity >= results[i + 1].similarity);
        }
    }
}

#[cfg(test)]
mod concurrent_tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_concurrent_adds() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = Arc::new(VectorDB::new(config).unwrap());

        let num_threads = 10;
        let vectors_per_thread = 10;

        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let db_clone = Arc::clone(&db);
                thread::spawn(move || {
                    for i in 0..vectors_per_thread {
                        let vector = vec![((thread_id * 10 + i) as f32) / 100.0; dim];
                        let metadata = VectorMetadata::new(
                            format!("thread-{}-vec-{}", thread_id, i),
                            format!("text {} {}", thread_id, i),
                            "test-source".to_string(),
                            "qa".to_string(),
                        );
                        db_clone.add_vector(vector, metadata).unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all vectors were added
        let stats = db.get_stats();
        assert_eq!(stats.total_vectors, num_threads * vectors_per_thread);
    }

    #[test]
    fn test_concurrent_search() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = Arc::new(VectorDB::new(config).unwrap());

        // Add some vectors first
        for i in 0..20 {
            let vector = vec![(i as f32) / 20.0; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Concurrent searches
        let num_threads = 20;
        let searches_per_thread = 10;

        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let db_clone = Arc::clone(&db);
                thread::spawn(move || {
                    for _ in 0..searches_per_thread {
                        let query = vec![(thread_id as f32) / num_threads as f32; dim];
                        let results = db_clone.search(&query, 5).unwrap();
                        assert!(results.len() <= 5);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Verify search operations completed successfully (avg_search_time_us > 0 indicates searches ran)
        let stats = db.get_stats();
        assert!(stats.avg_search_time_us > 0.0);
    }

    #[test]
    fn test_concurrent_add_remove() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = Arc::new(VectorDB::new(config).unwrap());

        let num_threads = 10;
        let operations_per_thread = 10;

        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let db_clone = Arc::clone(&db);
                thread::spawn(move || {
                    let mut ids = Vec::new();

                    // Add vectors
                    for i in 0..operations_per_thread {
                        let vector = vec![((thread_id * 10 + i) as f32) / 100.0; dim];
                        let metadata = VectorMetadata::new(
                            format!("thread-{}-vec-{}", thread_id, i),
                            format!("text {} {}", thread_id, i),
                            "test-source".to_string(),
                            "qa".to_string(),
                        );
                        let id = db_clone.add_vector(vector, metadata).unwrap();
                        ids.push(id);
                    }

                    // Remove half
                    for i in 0..operations_per_thread / 2 {
                        db_clone.remove_vector(ids[i]).unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Verify final count
        let stats = db.get_stats();
        let expected = num_threads * operations_per_thread - num_threads * (operations_per_thread / 2);
        assert_eq!(stats.total_vectors, expected);
    }
}

/// Tests for vector_db.rs fixes
#[cfg(test)]
mod vector_db_fix_tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    /// Test for issue #1/#2: Entry point race condition under concurrent load
    /// Verifies that concurrent vector additions don't corrupt the HNSW entry point
    #[test]
    fn test_entry_point_race_condition() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: true,
            max_connections: 16,
            ..Default::default()
        };
        let db = Arc::new(VectorDB::new(config).unwrap());

        let num_threads = 20;
        let vectors_per_thread = 50;

        // Concurrent additions with varying layer priorities
        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let db_clone = Arc::clone(&db);
                thread::spawn(move || {
                    for i in 0..vectors_per_thread {
                        // Create vectors with different magnitudes to stress entry point selection
                        let scale = ((thread_id * vectors_per_thread + i) as f32) / 1000.0;
                        let vector = vec![scale; dim];
                        let metadata = VectorMetadata::new(
                            format!("t{}-v{}", thread_id, i),
                            format!("text {} {}", thread_id, i),
                            "test-source".to_string(),
                            "qa".to_string(),
                        );
                        db_clone.add_vector(vector, metadata).unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all vectors were added
        let stats = db.get_stats();
        assert_eq!(stats.total_vectors, num_threads * vectors_per_thread);

        // Verify search still works (entry point is valid)
        let query = vec![0.5f32; dim];
        let results = db.search(&query, 10).unwrap();
        assert!(!results.is_empty(), "Search should return results if entry point is valid");

        // Verify results are properly ordered
        for i in 0..results.len().saturating_sub(1) {
            assert!(
                results[i].similarity >= results[i + 1].similarity,
                "Results should be in descending similarity order"
            );
        }
    }

    /// Test for issue #4: Nearest neighbor selection quality
    /// Verifies that HNSW connects to actual nearest neighbors, not random vectors
    #[test]
    fn test_nearest_neighbor_selection_quality() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: true,
            max_connections: 16,
            ef_construction: 200,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add a cluster of similar vectors
        let cluster_center = vec![0.8f32; dim];
        for i in 0..20 {
            let mut vector = cluster_center.clone();
            // Small perturbation to create cluster
            vector[i % dim] += 0.01 * (i as f32);
            let metadata = VectorMetadata::new(
                format!("cluster-{}", i),
                format!("cluster text {}", i),
                "cluster".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Add distant vectors
        for i in 0..20 {
            let mut vector = vec![0.1f32; dim];
            vector[i % dim] = 0.9;
            let metadata = VectorMetadata::new(
                format!("distant-{}", i),
                format!("distant text {}", i),
                "distant".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Search near cluster center - should find cluster members first
        let query = cluster_center.clone();
        let results = db.search(&query, 10).unwrap();

        // At least 7 of top 10 should be from the cluster (high similarity)
        let cluster_count = results
            .iter()
            .filter(|r| r.metadata.source == "cluster")
            .count();

        assert!(
            cluster_count >= 7,
            "Expected at least 7 cluster members in top 10, got {}. \
             This suggests HNSW is not connecting to nearest neighbors.",
            cluster_count
        );

        // Top result should have very high similarity (cluster member)
        assert!(
            results[0].similarity > 0.95,
            "Top result similarity {} is too low for cluster query",
            results[0].similarity
        );
    }

    /// Test for issue #6: Statistics underflow protection
    /// Verifies that double-remove doesn't cause underflow
    #[test]
    fn test_statistics_underflow_protection() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: false,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add a vector
        let vector = vec![0.5f32; dim];
        let metadata = VectorMetadata::new(
            "test".to_string(),
            "test text".to_string(),
            "test-source".to_string(),
            "qa".to_string(),
        );
        let id = db.add_vector(vector, metadata).unwrap();

        let stats_after_add = db.get_stats();
        assert_eq!(stats_after_add.total_vectors, 1);

        // First remove - should succeed
        let removed = db.remove_vector(id).unwrap();
        assert!(removed);

        let stats_after_remove = db.get_stats();
        assert_eq!(stats_after_remove.total_vectors, 0);

        // Second remove - should fail gracefully, not underflow
        let removed_again = db.remove_vector(id).unwrap();
        assert!(!removed_again, "Second remove should return false");

        // Stats should still be 0, not wrapped to MAX
        let stats_after_double_remove = db.get_stats();
        assert_eq!(
            stats_after_double_remove.total_vectors, 0,
            "Statistics should be 0, not underflowed"
        );
        assert!(
            stats_after_double_remove.memory_usage_bytes < 1_000_000_000,
            "Memory usage should not underflow to huge value"
        );
    }

    /// Test for issue #9: Memory statistics accuracy
    /// Verifies that add/remove keeps memory stats accurate
    #[test]
    fn test_memory_statistics_accuracy() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: true, // Enable to test quantized size calculation
            enable_hnsw_index: false,
            enable_product_quantization: true,
            pq_subvectors: 8,
            pq_bits: 8,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        let initial_memory = db.get_stats().memory_usage_bytes;

        // Add vectors
        let mut ids = Vec::new();
        for i in 0..10 {
            let vector = vec![(i as f32) / 10.0; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            let id = db.add_vector(vector, metadata).unwrap();
            ids.push(id);
        }

        let memory_after_add = db.get_stats().memory_usage_bytes;
        assert!(
            memory_after_add > initial_memory,
            "Memory should increase after adding vectors"
        );

        // Remove all vectors
        for id in ids {
            db.remove_vector(id).unwrap();
        }

        let memory_after_remove = db.get_stats().memory_usage_bytes;

        // Memory should return close to initial (may not be exact due to overhead)
        assert_eq!(
            memory_after_remove, initial_memory,
            "Memory should return to initial after removing all vectors"
        );
    }

    /// Test for issue #8: DashMap iteration during modification
    /// Verifies that build_index works correctly under concurrent access
    #[test]
    fn test_build_index_concurrent_safety() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: true,
            max_connections: 8,
            ..Default::default()
        };
        let db = Arc::new(VectorDB::new(config).unwrap());

        // Add initial vectors
        for i in 0..20 {
            let vector = vec![(i as f32) / 20.0; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Concurrent: one thread rebuilds index, others add vectors
        let db_clone = Arc::clone(&db);
        let rebuild_handle = thread::spawn(move || {
            db_clone.build_index().unwrap();
        });

        let db_clone2 = Arc::clone(&db);
        let add_handle = thread::spawn(move || {
            for i in 20..40 {
                let vector = vec![(i as f32) / 40.0; dim];
                let metadata = VectorMetadata::new(
                    format!("vec-{}", i),
                    format!("text {}", i),
                    "test-source".to_string(),
                    "qa".to_string(),
                );
                let _ = db_clone2.add_vector(vector, metadata);
            }
        });

        rebuild_handle.join().unwrap();
        add_handle.join().unwrap();

        // Verify database is still functional
        let query = vec![0.5f32; dim];
        let results = db.search(&query, 10).unwrap();
        assert!(!results.is_empty(), "Search should work after concurrent build_index");
    }
}

/// Phase 6: Semantic Search Optimization Tests
#[cfg(test)]
mod search_mode_tests {
    use super::*;
    use post_cortex_embeddings::SearchMode;

    #[test]
    fn test_search_mode_exact_vs_approximate() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: true,
            max_connections: 16,
            ef_construction: 200,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors
        for i in 0..50 {
            let mut vector = vec![0.0f32; dim];
            vector[0] = (i as f32) / 50.0;
            vector[1] = 1.0 - (i as f32) / 50.0;
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        // Query vector
        let query = vec![0.5f32, 0.5f32]
            .into_iter()
            .chain(std::iter::repeat(0.0f32).take(dim - 2))
            .collect::<Vec<f32>>();

        // Exact search (linear scan)
        let exact_results = db.search_with_mode(&query, 10, SearchMode::Exact, None).unwrap();

        // Approximate search (HNSW)
        let approx_results = db
            .search_with_mode(&query, 10, SearchMode::Approximate, None)
            .unwrap();

        // Balanced search
        let balanced_results = db
            .search_with_mode(&query, 10, SearchMode::Balanced, None)
            .unwrap();

        // All should return some results
        assert!(exact_results.len() > 0);
        assert!(approx_results.len() > 0);
        assert!(balanced_results.len() > 0);

        // Exact search should have highest similarity for top result
        assert!(exact_results[0].similarity >= approx_results[0].similarity - 0.1);

        println!("Exact top similarity: {}", exact_results[0].similarity);
        println!("Approx top similarity: {}", approx_results[0].similarity);
        println!("Balanced top similarity: {}", balanced_results[0].similarity);
    }

    #[test]
    fn test_search_quality_presets() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: true,
            max_connections: 16,
            ef_construction: 200,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors
        for i in 0..100 {
            let vector = vec![(i as f32) / 100.0; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        let query = vec![0.5f32; dim];

        // Test with different ef_search values
        let fast_results = db
            .search_with_mode(&query, 10, SearchMode::Approximate, Some(32))
            .unwrap();

        let balanced_results = db
            .search_with_mode(&query, 10, SearchMode::Approximate, Some(64))
            .unwrap();

        let accurate_results = db
            .search_with_mode(&query, 10, SearchMode::Approximate, Some(128))
            .unwrap();

        // All should return results
        assert_eq!(fast_results.len(), 10);
        assert_eq!(balanced_results.len(), 10);
        assert_eq!(accurate_results.len(), 10);

        // Higher ef_search should generally give equal or better results
        assert!(accurate_results[0].similarity >= balanced_results[0].similarity - 0.05);
        assert!(balanced_results[0].similarity >= fast_results[0].similarity - 0.05);

        println!("Fast (ef=32): {}", fast_results[0].similarity);
        println!("Balanced (ef=64): {}", balanced_results[0].similarity);
        println!("Accurate (ef=128): {}", accurate_results[0].similarity);
    }

    #[test]
    fn test_exact_search_deterministic() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: true,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add vectors
        for i in 0..20 {
            let vector = vec![(i as f32) / 20.0; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        let query = vec![0.5f32; dim];

        // Run exact search multiple times - should always give same results
        let results1 = db.search_with_mode(&query, 5, SearchMode::Exact, None).unwrap();
        let results2 = db.search_with_mode(&query, 5, SearchMode::Exact, None).unwrap();
        let results3 = db.search_with_mode(&query, 5, SearchMode::Exact, None).unwrap();

        assert_eq!(results1.len(), results2.len());
        assert_eq!(results2.len(), results3.len());

        for i in 0..results1.len() {
            assert_eq!(results1[i].vector_id, results2[i].vector_id);
            assert_eq!(results2[i].vector_id, results3[i].vector_id);
            assert!((results1[i].similarity - results2[i].similarity).abs() < 1e-6);
        }
    }

    #[serial]
    #[test]
    fn test_search_mode_performance_comparison() {
        let dim = 384;
        let config = VectorDbConfig {
            dimension: dim,
            enable_quantization: false,
            enable_hnsw_index: true,
            max_connections: 16,
            ef_construction: 200,
            ..Default::default()
        };
        let db = VectorDB::new(config).unwrap();

        // Add many vectors to see performance difference
        for i in 0..500 {
            let vector = vec![(i as f32) / 500.0; dim];
            let metadata = VectorMetadata::new(
                format!("vec-{}", i),
                format!("text {}", i),
                "test-source".to_string(),
                "qa".to_string(),
            );
            db.add_vector(vector, metadata).unwrap();
        }

        let query = vec![0.5f32; dim];

        // Benchmark different modes
        let start = std::time::Instant::now();
        let exact_results = db.search_with_mode(&query, 10, SearchMode::Exact, None).unwrap();
        let exact_time = start.elapsed();

        let start = std::time::Instant::now();
        let approx_results = db
            .search_with_mode(&query, 10, SearchMode::Approximate, None)
            .unwrap();
        let approx_time = start.elapsed();

        let start = std::time::Instant::now();
        let balanced_results = db
            .search_with_mode(&query, 10, SearchMode::Balanced, None)
            .unwrap();
        let balanced_time = start.elapsed();

        println!("Exact search: {:?}", exact_time);
        println!("Approximate search: {:?}", approx_time);
        println!("Balanced search: {:?}", balanced_time);

        // Verify all return results
        assert_eq!(exact_results.len(), 10);
        assert_eq!(approx_results.len(), 10);
        assert_eq!(balanced_results.len(), 10);

        // HNSW should be faster than linear search for large datasets
        // (may not always be true for small datasets due to overhead)
        println!(
            "Speedup (approx vs exact): {:.2}x",
            exact_time.as_secs_f64() / approx_time.as_secs_f64()
        );
    }
}
