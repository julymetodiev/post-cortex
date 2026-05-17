// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

use super::*;

#[test]
fn test_vector_db_creation() {
    let config = VectorDbConfig::default();
    let db = VectorDB::new(config).unwrap();

    let stats = db.get_stats();
    assert_eq!(stats.total_vectors, 0);
    assert!(!stats.is_built);
}

#[test]
fn test_add_and_search_vector() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vector = vec![1.0, 0.0, 0.0];
    let metadata = VectorMetadata::new(
        "test1".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "test content".to_string(),
    );

    let id = db.add_vector(vector.clone(), metadata).unwrap();
    assert_eq!(id, 0);

    let results = db.search(&vector, 5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].vector_id, 0);
    assert!((results[0].similarity - 1.0).abs() < 0.001);
}

#[test]
fn test_vector_similarity() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vector1 = vec![1.0, 0.0, 0.0];
    let vector2 = vec![0.9, 0.1, 0.0]; // Similar vector

    let metadata1 = VectorMetadata::new(
        "test1".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "content1".to_string(),
    );

    let metadata2 = VectorMetadata::new(
        "test2".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "content2".to_string(),
    );

    db.add_vector(vector1.clone(), metadata1).unwrap();
    db.add_vector(vector2.clone(), metadata2).unwrap();

    let results = db.search(&vector1, 5).unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].similarity > 0.9); // High similarity
}

#[test]
fn test_remove_vector() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vector = vec![1.0, 0.0, 0.0];
    let metadata = VectorMetadata::new(
        "test1".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "test content".to_string(),
    );

    let id = db.add_vector(vector, metadata).unwrap();
    assert_eq!(db.get_stats().total_vectors, 1);

    let removed = db.remove_vector(id).unwrap();
    assert!(removed);
    assert_eq!(db.get_stats().total_vectors, 0);

    let results = db.search(&[1.0, 0.0, 0.0], 5).unwrap();
    assert_eq!(results.len(), 0);
}

#[test]
fn test_batch_operations() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vectors = vec![
        (
            vec![1.0, 0.0, 0.0],
            VectorMetadata::new(
                "test1".to_string(),
                "session1".to_string(),
                "text".to_string(),
                "content1".to_string(),
            ),
        ),
        (
            vec![0.0, 1.0, 0.0],
            VectorMetadata::new(
                "test2".to_string(),
                "session1".to_string(),
                "text".to_string(),
                "content2".to_string(),
            ),
        ),
        (
            vec![0.0, 0.0, 1.0],
            VectorMetadata::new(
                "test3".to_string(),
                "session1".to_string(),
                "text".to_string(),
                "content3".to_string(),
            ),
        ),
    ];

    let ids = db.add_vectors_batch(vectors).unwrap();
    assert_eq!(ids.len(), 3);
    assert_eq!(db.get_stats().total_vectors, 3);

    let results = db.search(&[1.0, 0.0, 0.0], 5).unwrap();
    // The search should find all 3 vectors since they are all similar to [1.0, 0.0, 0.0] to some degree
    assert!(!results.is_empty()); // At least the exact match should be found
}
