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

use super::types::EMBEDDING_DIMENSION;
use super::*;
use chrono::Utc;
use std::collections::HashMap;
use tempfile::tempdir;
use uuid::Uuid;

use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::VectorMetadata;
use post_cortex_ports::storage::{SessionStore, VectorStore};

#[tokio::test]
async fn test_rocksdb_session_operations() {
    let temp_dir = tempdir().expect("Failed to create temp directory");
    let storage = RealRocksDBStorage::new(temp_dir.path())
        .await
        .expect("Failed to create RocksDB storage");

    // Create test session
    let session_id = Uuid::new_v4();
    let session = ActiveSession::new(
        session_id,
        Some("Test Session".to_string()),
        Some("A test session for RocksDB storage operations".to_string()),
        chrono::Utc::now(),
    );

    // Save session
    storage
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .expect("Failed to save session");

    // Load session
    let loaded_session = storage
        .load_session(session.id())
        .await
        .expect("Failed to load session")
        .session;
    assert_eq!(session.id(), loaded_session.id());

    // Check if session exists
    assert!(
        storage
            .session_exists(session.id())
            .await
            .expect("Failed to check session existence")
    );

    // Delete session
    storage
        .delete_session(session.id())
        .await
        .expect("Failed to delete session");
    assert!(
        !storage
            .session_exists(session.id())
            .await
            .expect("Failed to check session existence after deletion")
    );
}

fn vector_metadata(id: &str, source: &str, content_type: &str) -> VectorMetadata {
    VectorMetadata {
        id: id.to_string(),
        text: format!("text for {id}").into(),
        source: source.to_string(),
        content_type: content_type.to_string(),
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn test_rocksdb_embedding_dimension_validation() {
    let temp_dir = tempdir().expect("Failed to create temp directory");
    let storage = RealRocksDBStorage::new_with_dimension(temp_dir.path(), EMBEDDING_DIMENSION)
        .await
        .expect("Failed to create RocksDB storage");

    let metadata = vector_metadata("test-id", &Uuid::new_v4().to_string(), "qa");

    // Valid dimension (384)
    let valid_vector = vec![0.1f32; EMBEDDING_DIMENSION];
    let result = storage
        .upsert_vectors(vec![(valid_vector, metadata.clone())])
        .await;
    assert!(result.is_ok());

    // Invalid dimension (wrong size)
    let invalid_vector = vec![0.1f32; 100];
    let result = storage
        .upsert_vectors(vec![(invalid_vector, metadata)])
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains(&format!(
        "embedding dimension: expected {EMBEDDING_DIMENSION}, got 100"
    )));
}

/// The default Model2Vec model emits 256-dim vectors; they used to be
/// rejected by a hard-coded 384 check, so nothing was ever persisted.
#[tokio::test]
async fn test_rocksdb_accepts_configured_dimension() {
    let temp_dir = tempdir().unwrap();
    let source = Uuid::new_v4().to_string();

    let storage = RealRocksDBStorage::new_with_dimension(temp_dir.path(), 256)
        .await
        .unwrap();
    storage
        .upsert_vectors(vec![(vec![0.5; 256], vector_metadata("a", &source, "qa"))])
        .await
        .expect("256-dim vector must be accepted when configured");
    assert!(
        storage
            .upsert_vectors(vec![(vec![0.5; 384], vector_metadata("b", &source, "qa"))])
            .await
            .is_err()
    );
    drop(storage);

    // `new` enforces nothing but non-emptiness (export/import/migrate).
    let storage = RealRocksDBStorage::new(temp_dir.path()).await.unwrap();
    storage
        .upsert_vectors(vec![(vec![0.5; 384], vector_metadata("b", &source, "qa"))])
        .await
        .unwrap();
    assert!(
        storage
            .upsert_vectors(vec![(Vec::new(), vector_metadata("c", &source, "qa"))])
            .await
            .is_err()
    );
    assert_eq!(storage.count_session_embeddings(&source).await.unwrap(), 2);
}

#[tokio::test]
async fn test_rocksdb_vector_storage_without_resident_index() {
    let temp_dir = tempdir().unwrap();
    let s1 = Uuid::new_v4().to_string();
    let s2 = Uuid::new_v4().to_string();

    {
        let storage = RealRocksDBStorage::new_with_dimension(temp_dir.path(), 3)
            .await
            .unwrap();
        storage
            .upsert_vectors(vec![
                (vec![1.0, 0.0, 0.0], vector_metadata("x", &s1, "qa")),
                (vec![0.0, 1.0, 0.0], vector_metadata("y", &s1, "decision")),
                (vec![0.9, 0.1, 0.0], vector_metadata("entity:z", &s2, "qa")),
            ])
            .await
            .unwrap();
    }

    // Reopen: everything is served from the persisted rows.
    let storage = RealRocksDBStorage::new_with_dimension(temp_dir.path(), 3)
        .await
        .unwrap();
    assert_eq!(storage.total_count().await.unwrap(), 3);
    assert!(storage.has_session_embeddings(&s1).await.unwrap());
    assert_eq!(storage.count_session_embeddings(&s1).await.unwrap(), 2);
    assert!(!storage.has_session_embeddings("nope").await.unwrap());

    // Removal is scoped to the session.
    assert_eq!(storage.delete_vectors(&s2, &["x".into()]).await.unwrap(), 0);
    assert_eq!(
        storage
            .delete_vectors(&s1, &["x".into(), "entity:z".into()])
            .await
            .unwrap(),
        1
    );
    assert_eq!(storage.total_count().await.unwrap(), 2);
    assert_eq!(storage.get_all_vectors().await.unwrap().len(), 2);
}
