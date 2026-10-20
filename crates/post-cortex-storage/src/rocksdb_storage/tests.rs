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

use crate::traits::{GraphStorage, VectorStorage};
use post_cortex_core::core::context_update::{EntityData, EntityRelationship, RelationType};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::VectorMetadata;

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
    );

    // Save session
    storage
        .save_session(&session)
        .await
        .expect("Failed to save session");

    // Load session
    let loaded_session = storage
        .load_session(session.id())
        .await
        .expect("Failed to load session");
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

#[tokio::test]
async fn test_rocksdb_graph_storage_entities() {
    use post_cortex_core::core::context_update::EntityType;

    let temp_dir = tempdir().expect("Failed to create temp directory");
    let storage = RealRocksDBStorage::new(temp_dir.path())
        .await
        .expect("Failed to create RocksDB storage");

    let session_id = Uuid::new_v4();

    // Create test entity
    let entity = EntityData {
        aliases: Vec::new(),
        name: "Rust".to_string(),
        entity_type: EntityType::Technology,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 5,
        importance_score: 0.8,
        description: Some("A systems programming language".to_string()),
    };

    // Upsert entity
    storage
        .upsert_entity(session_id, &entity)
        .await
        .expect("Failed to upsert entity");

    // Get entity
    let loaded = storage
        .get_entity(session_id, "Rust")
        .await
        .expect("Failed to get entity");
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.name, "Rust");
    assert_eq!(loaded.mention_count, 5);

    // List entities
    let entities = storage
        .list_entities(session_id)
        .await
        .expect("Failed to list entities");
    assert_eq!(entities.len(), 1);

    // Delete entity
    storage
        .delete_entity(session_id, "Rust")
        .await
        .expect("Failed to delete entity");

    let deleted = storage
        .get_entity(session_id, "Rust")
        .await
        .expect("Failed to check deleted entity");
    assert!(deleted.is_none());
}

#[tokio::test]
async fn test_rocksdb_graph_storage_relationships() {
    use post_cortex_core::core::context_update::EntityType;

    let temp_dir = tempdir().expect("Failed to create temp directory");
    let storage = RealRocksDBStorage::new(temp_dir.path())
        .await
        .expect("Failed to create RocksDB storage");

    let session_id = Uuid::new_v4();

    // Create test entities
    let rust = EntityData {
        aliases: Vec::new(),
        name: "Rust".to_string(),
        entity_type: EntityType::Technology,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 5,
        importance_score: 0.8,
        description: None,
    };

    let tokio = EntityData {
        aliases: Vec::new(),
        name: "Tokio".to_string(),
        entity_type: EntityType::Technology,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 3,
        importance_score: 0.6,
        description: None,
    };

    storage.upsert_entity(session_id, &rust).await.unwrap();
    storage.upsert_entity(session_id, &tokio).await.unwrap();

    // Create relationship
    let relationship = EntityRelationship {
        from_entity: "Tokio".to_string(),
        to_entity: "Rust".to_string(),
        relation_type: RelationType::DependsOn,
        context: "Tokio is built on Rust".to_string(),
    };

    storage
        .create_relationship(session_id, &relationship)
        .await
        .expect("Failed to create relationship");

    // Find related entities
    let related = storage
        .find_related_entities(session_id, "Rust")
        .await
        .expect("Failed to find related entities");
    assert_eq!(related.len(), 1);
    assert_eq!(related[0], "Tokio");

    // Find related by type
    let related_by_type = storage
        .find_related_by_type(session_id, "Rust", &RelationType::DependsOn)
        .await
        .expect("Failed to find related by type");
    assert_eq!(related_by_type.len(), 1);
}

#[tokio::test]
async fn test_rocksdb_graph_storage_shortest_path() {
    use post_cortex_core::core::context_update::EntityType;

    let temp_dir = tempdir().expect("Failed to create temp directory");
    let storage = RealRocksDBStorage::new(temp_dir.path())
        .await
        .expect("Failed to create RocksDB storage");

    let session_id = Uuid::new_v4();

    // Create chain: A -> B -> C
    for name in ["A", "B", "C"] {
        let entity = EntityData {
            aliases: Vec::new(),
            name: name.to_string(),
            entity_type: EntityType::Concept,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 1,
            importance_score: 0.5,
            description: None,
        };
        storage.upsert_entity(session_id, &entity).await.unwrap();
    }

    // A -> B
    storage
        .create_relationship(
            session_id,
            &EntityRelationship {
                from_entity: "A".to_string(),
                to_entity: "B".to_string(),
                relation_type: RelationType::LeadsTo,
                context: String::new(),
            },
        )
        .await
        .unwrap();

    // B -> C
    storage
        .create_relationship(
            session_id,
            &EntityRelationship {
                from_entity: "B".to_string(),
                to_entity: "C".to_string(),
                relation_type: RelationType::LeadsTo,
                context: String::new(),
            },
        )
        .await
        .unwrap();

    // Find shortest path A -> C
    let path = storage
        .find_shortest_path(session_id, "A", "C")
        .await
        .expect("Failed to find shortest path");
    assert!(path.is_some());
    let path = path.unwrap();
    assert_eq!(path, vec!["A", "B", "C"]);

    // No path exists to unconnected node
    let entity_d = EntityData {
        aliases: Vec::new(),
        name: "D".to_string(),
        entity_type: EntityType::Concept,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 1,
        importance_score: 0.5,
        description: None,
    };
    storage.upsert_entity(session_id, &entity_d).await.unwrap();

    let no_path = storage
        .find_shortest_path(session_id, "A", "D")
        .await
        .expect("Failed to check no path");
    assert!(no_path.is_none());
}

fn vector_metadata(id: &str, source: &str, content_type: &str) -> VectorMetadata {
    VectorMetadata {
        id: id.to_string(),
        text: format!("text for {id}"),
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
    let result = storage.add_vector(valid_vector, metadata.clone()).await;
    assert!(result.is_ok());

    // Invalid dimension (wrong size)
    let invalid_vector = vec![0.1f32; 100];
    let result = storage.add_vector(invalid_vector, metadata).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Invalid embedding dimension")
    );
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
        .add_vector(vec![0.5; 256], vector_metadata("a", &source, "qa"))
        .await
        .expect("256-dim vector must be accepted when configured");
    assert!(
        storage
            .add_vector(vec![0.5; 384], vector_metadata("b", &source, "qa"))
            .await
            .is_err()
    );
    drop(storage);

    // `new` enforces nothing but non-emptiness (export/import/migrate).
    let storage = RealRocksDBStorage::new(temp_dir.path()).await.unwrap();
    storage
        .add_vector(vec![0.5; 384], vector_metadata("b", &source, "qa"))
        .await
        .unwrap();
    assert!(
        storage
            .add_vector(Vec::new(), vector_metadata("c", &source, "qa"))
            .await
            .is_err()
    );
    assert_eq!(storage.count_session_embeddings(&source).await, 2);
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
        let ids = storage
            .add_vectors_batch(vec![
                (vec![1.0, 0.0, 0.0], vector_metadata("x", &s1, "qa")),
                (vec![0.0, 1.0, 0.0], vector_metadata("y", &s1, "decision")),
                (vec![0.9, 0.1, 0.0], vector_metadata("entity:z", &s2, "qa")),
            ])
            .await
            .unwrap();
        assert_eq!(ids, vec!["x", "y", "entity:z"]);
    }

    // Reopen: everything is served from the persisted rows.
    let storage = RealRocksDBStorage::new_with_dimension(temp_dir.path(), 3)
        .await
        .unwrap();
    assert_eq!(storage.total_count().await, 3);
    assert!(storage.has_session_embeddings(&s1).await);
    assert_eq!(storage.count_session_embeddings(&s1).await, 2);
    assert!(!storage.has_session_embeddings("nope").await);

    let top = storage.search(&[1.0, 0.0, 0.0], 2).await.unwrap();
    let top_ids: Vec<&str> = top.iter().map(|m| m.metadata.id.as_str()).collect();
    assert_eq!(top_ids, vec!["x", "entity:z"]);
    assert!(top[0].similarity >= top[1].similarity);

    let in_s1 = storage
        .search_in_session(&[0.0, 1.0, 0.0], 5, &s1)
        .await
        .unwrap();
    assert_eq!(in_s1.len(), 2);
    assert_eq!(in_s1[0].metadata.id, "y");

    let by_type = storage
        .search_by_content_type(&[1.0, 0.0, 0.0], 5, "decision")
        .await
        .unwrap();
    assert_eq!(by_type.len(), 1);
    assert_eq!(by_type[0].metadata.id, "y");

    // Removal by content id (any session), then scoped removal.
    assert!(storage.remove_vector("entity:z").await.unwrap());
    assert!(!storage.remove_vector("entity:z").await.unwrap());
    assert!(!storage.remove_session_vector(&s2, "x").await.unwrap());
    assert!(storage.remove_session_vector(&s1, "x").await.unwrap());
    assert_eq!(storage.total_count().await, 1);
    assert_eq!(storage.get_all_vectors().await.unwrap().len(), 1);
}
