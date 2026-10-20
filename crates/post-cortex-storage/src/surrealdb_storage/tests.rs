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

use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

use super::SurrealDBStorage;
use crate::traits::{GraphStorage, Storage};
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityData, EntityRelationship, EntityType, RelationType, UpdateContent,
    UpdateType,
};
use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::{ActiveSession, UserPreferences};

/// Compute cosine similarity between two vectors (used in tests)
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}

#[tokio::test]
async fn test_surrealdb_session_operations() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .expect("Failed to create SurrealDB storage");

    // Create test session
    let session_id = Uuid::new_v4();
    let session = ActiveSession::new(
        session_id,
        Some("Test Session".to_string()),
        Some("A test session".to_string()),
    );

    // Save session
    storage
        .save_session(&session)
        .await
        .expect("Failed to save session");

    // Load session
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session");
    assert_eq!(session.id(), loaded.id());

    // Check exists
    assert!(storage.session_exists(session_id).await.unwrap());

    // Delete
    storage
        .delete_session(session_id)
        .await
        .expect("Failed to delete session");
    assert!(!storage.session_exists(session_id).await.unwrap());
}

#[tokio::test]
async fn test_entity_operations() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .expect("Failed to create SurrealDB storage");

    let session_id = Uuid::new_v4();
    let entity = EntityData {
        aliases: Vec::new(),
        name: "rust".to_string(),
        entity_type: EntityType::Technology,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: Some("Rust programming language".to_string()),
    };

    // Upsert entity
    storage
        .upsert_entity(session_id, &entity)
        .await
        .expect("Failed to upsert entity");

    // Get entity
    let loaded = storage
        .get_entity(session_id, "rust")
        .await
        .expect("Failed to get entity");
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().name, "rust");

    // List entities
    let entities = storage
        .list_entities(session_id)
        .await
        .expect("Failed to list entities");
    assert_eq!(entities.len(), 1);

    // Delete entity
    storage
        .delete_entity(session_id, "rust")
        .await
        .expect("Failed to delete entity");
    let deleted = storage
        .get_entity(session_id, "rust")
        .await
        .expect("Failed to check deleted");
    assert!(deleted.is_none());
}

#[test]
fn test_cosine_similarity() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![1.0, 0.0, 0.0];
    assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.0001);

    let c = vec![0.0, 1.0, 0.0];
    assert!((cosine_similarity(&a, &c) - 0.0).abs() < 0.0001);

    let d = vec![-1.0, 0.0, 0.0];
    assert!((cosine_similarity(&a, &d) - (-1.0)).abs() < 0.0001);
}

/// Test remote connection to Docker SurrealDB (requires running Docker container)
/// Run with: cargo test --features surrealdb-storage test_remote_surrealdb -- --ignored
#[tokio::test]
#[ignore = "Requires running Docker SurrealDB at localhost:8000"]
async fn test_remote_surrealdb_connection() {
    // Connect to Docker SurrealDB (default: root/root)
    let storage = SurrealDBStorage::new("localhost:8000", Some("root"), Some("root"), None, None)
        .await
        .expect("Failed to connect to remote SurrealDB");

    // Create test session
    let session_id = Uuid::new_v4();
    let session = ActiveSession::new(
        session_id,
        Some("Remote Test Session".to_string()),
        Some("Testing remote connection".to_string()),
    );

    // Save session
    storage
        .save_session(&session)
        .await
        .expect("Failed to save session to remote");

    // Load session
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session from remote");
    assert_eq!(session.id(), loaded.id());
    assert_eq!(
        loaded.name().clone(),
        Some("Remote Test Session".to_string())
    );

    // Clean up
    storage
        .delete_session(session_id)
        .await
        .expect("Failed to delete session from remote");

    println!("Remote SurrealDB connection test passed!");
}

// ========================================================================
// Normalized Storage Tests
// ========================================================================

/// Helper to create test session with pre-populated data
fn create_test_session_with_updates(
    session_id: Uuid,
    name: &str,
    updates: Vec<ContextUpdate>,
    entity_graph: SimpleEntityGraph,
) -> ActiveSession {
    ActiveSession::from_components(
        session_id,
        Some(name.to_string()),
        Some("Test session".to_string()),
        Utc::now(),
        Utc::now(),
        UserPreferences {
            auto_save_enabled: true,
            context_retention_days: 30,
            max_hot_context_size: 50,
            auto_summary_threshold: 100,
            important_keywords: Vec::new(),
        },
        updates.clone(), // hot_context_vec
        Vec::new(),      // warm_context
        Vec::new(),      // cold_context
        StructuredContext::default(),
        updates,        // incremental_updates
        HashMap::new(), // code_references
        Vec::new(),     // change_history
        entity_graph,
        Vec::new(), // vectorized_update_ids
    )
}

#[tokio::test]
async fn test_normalized_context_updates_roundtrip() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .expect("Failed to create SurrealDB storage");

    let session_id = Uuid::new_v4();

    // Create context updates
    let update1 = ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: UpdateType::QuestionAnswered,
        content: UpdateContent {
            title: "How does SurrealDB work?".to_string(),
            description: "SurrealDB is a multi-model database".to_string(),
            details: vec!["Graph support".to_string(), "Vector search".to_string()],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: true,
        creates_entities: vec!["SurrealDB".to_string()],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    };

    let update2 = ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: "Use normalized storage".to_string(),
            description: "Store context updates in separate table".to_string(),
            details: vec!["No JSON blobs".to_string()],
            examples: vec![],
            implications: vec!["Better queryability".to_string()],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec!["NormalizedStorage".to_string()],
        creates_relationships: vec![],
        references_entities: vec!["SurrealDB".to_string()],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    };

    // Create session with updates
    let session = create_test_session_with_updates(
        session_id,
        "Normalized Test",
        vec![update1, update2],
        SimpleEntityGraph::new(),
    );

    // Save session (should save updates to context_update table)
    storage
        .save_session(&session)
        .await
        .expect("Failed to save session");

    // Verify updates are in context_update table
    let loaded_updates = storage
        .load_session_updates(session_id)
        .await
        .expect("Failed to load updates");
    assert_eq!(
        loaded_updates.len(),
        2,
        "Should have 2 updates in normalized table"
    );

    // Load session (should rebuild from normalized tables)
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session");

    // Verify hot_context was rebuilt
    let hot_updates: Vec<_> = loaded.hot_context.iter();
    assert_eq!(hot_updates.len(), 2, "Hot context should have 2 updates");

    // Verify update content is preserved
    let found_qa = hot_updates.iter().any(|u| {
        u.update_type == UpdateType::QuestionAnswered
            && u.content.title == "How does SurrealDB work?"
    });
    assert!(found_qa, "Should find QuestionAnswered update");

    let found_decision = hot_updates.iter().any(|u| {
        u.update_type == UpdateType::DecisionMade && u.content.title == "Use normalized storage"
    });
    assert!(found_decision, "Should find DecisionMade update");
}

#[tokio::test]
async fn test_normalized_entities_and_relationships() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .expect("Failed to create SurrealDB storage");

    let session_id = Uuid::new_v4();

    // Create entity graph with entities and relationships
    let mut entity_graph = SimpleEntityGraph::new();
    entity_graph.add_or_update_entity(
        "Rust".to_string(),
        EntityType::Technology,
        Utc::now(),
        "Programming language",
    );
    entity_graph.add_or_update_entity(
        "SurrealDB".to_string(),
        EntityType::Technology,
        Utc::now(),
        "Multi-model database",
    );

    // Add relationship
    let relationship = EntityRelationship {
        from_entity: "Rust".to_string(),
        to_entity: "SurrealDB".to_string(),
        relation_type: RelationType::Implements,
        context: "Rust client for SurrealDB".to_string(),
    };
    entity_graph.add_relationship(relationship);

    // Create session with entity graph
    let session =
        create_test_session_with_updates(session_id, "Graph Test", Vec::new(), entity_graph);

    // Save session
    storage
        .save_session(&session)
        .await
        .expect("Failed to save session");

    // Verify entities in native table
    let entities = storage
        .list_entities(session_id)
        .await
        .expect("Failed to list entities");
    assert_eq!(entities.len(), 2, "Should have 2 entities");

    let rust_entity = entities.iter().find(|e| e.name == "Rust");
    assert!(rust_entity.is_some(), "Should find Rust entity");
    assert_eq!(rust_entity.unwrap().entity_type, EntityType::Technology);

    // Verify relationships via load_all_relationships
    let relationships = storage
        .load_all_relationships(session_id)
        .await
        .expect("Failed to load relationships");
    assert_eq!(relationships.len(), 1, "Should have 1 relationship");
    assert_eq!(relationships[0].from_entity, "Rust");
    assert_eq!(relationships[0].to_entity, "SurrealDB");
    assert_eq!(relationships[0].relation_type, RelationType::Implements);
    assert_eq!(relationships[0].context, "Rust client for SurrealDB");

    // Load session and verify graph is rebuilt
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session");

    let loaded_entities = loaded.entity_graph.get_all_entities();
    assert_eq!(
        loaded_entities.len(),
        2,
        "Loaded session should have 2 entities"
    );

    let loaded_relationships = loaded.entity_graph.get_all_relationships();
    assert_eq!(
        loaded_relationships.len(),
        1,
        "Loaded session should have 1 relationship"
    );
    assert_eq!(loaded_relationships[0].context, "Rust client for SurrealDB");
}

#[test]
fn test_extract_code_references() {
    use post_cortex_core::core::context_update::CodeReference as CoreCodeRef;

    let updates = vec![
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::CodeChanged,
            content: UpdateContent {
                title: "Fix bug".to_string(),
                description: "Fixed null pointer".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: Some(CoreCodeRef {
                file_path: "src/main.rs".to_string(),
                start_line: 10,
                end_line: 20,
                code_snippet: "fn main() {}".to_string(),
                commit_hash: Some("abc123".to_string()),
                branch: Some("main".to_string()),
                change_description: "Fixed bug".to_string(),
            }),
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::CodeChanged,
            content: UpdateContent {
                title: "Add feature".to_string(),
                description: "Added logging".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: Some(CoreCodeRef {
                file_path: "src/main.rs".to_string(), // Same file, second reference
                start_line: 30,
                end_line: 40,
                code_snippet: "fn log() {}".to_string(),
                commit_hash: Some("def456".to_string()),
                branch: Some("feature".to_string()),
                change_description: "Added logging".to_string(),
            }),
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::QuestionAnswered,
            content: UpdateContent {
                title: "Question".to_string(),
                description: "Answer".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: None, // No code reference
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
    ];

    let refs = SurrealDBStorage::extract_code_references(&updates);

    // Should have 1 file with 2 references
    assert_eq!(refs.len(), 1, "Should have 1 file path");
    assert!(refs.contains_key("src/main.rs"), "Should have src/main.rs");

    let main_refs = refs.get("src/main.rs").unwrap();
    assert_eq!(
        main_refs.len(),
        2,
        "Should have 2 references for src/main.rs"
    );

    // Verify both references are captured
    assert!(
        main_refs.iter().any(|r| r.start_line == 10),
        "Should have first reference"
    );
    assert!(
        main_refs.iter().any(|r| r.start_line == 30),
        "Should have second reference"
    );
}

#[test]
fn test_extract_change_history() {
    let updates = vec![
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::CodeChanged,
            content: UpdateContent {
                title: "Refactor storage".to_string(),
                description: "Split into modules".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::QuestionAnswered, // Not CodeChanged
            content: UpdateContent {
                title: "Question".to_string(),
                description: "Answer".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::CodeChanged,
            content: UpdateContent {
                title: "Add tests".to_string(),
                description: "Unit tests for helpers".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
    ];

    let history = SurrealDBStorage::extract_change_history(&updates);

    // Should only include CodeChanged updates; description is taken straight from
    // update.content.description (matches active_session::record_change).
    assert_eq!(history.len(), 2, "Should have 2 change records");
    assert!(
        history
            .iter()
            .any(|r| r.description == "Split into modules")
    );
    assert!(
        history
            .iter()
            .any(|r| r.description == "Unit tests for helpers")
    );
    assert!(!history.iter().any(|r| r.description == "Answer"));
}

#[test]
fn test_rebuild_structured_context() {
    let updates = vec![
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::DecisionMade,
            content: UpdateContent {
                title: "Use SurrealDB".to_string(),
                description: "For graph and vector storage".to_string(),
                details: vec!["Option A".to_string(), "Option B".to_string()],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: true,
            creates_entities: vec!["SurrealDB".to_string()],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::QuestionAnswered,
            content: UpdateContent {
                title: "How to normalize?".to_string(),
                description: "Store in separate tables".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec!["SurrealDB".to_string()],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        },
    ];

    let context = SurrealDBStorage::rebuild_structured_context(&updates);

    // Should have 1 decision
    assert_eq!(context.key_decisions.len(), 1, "Should have 1 decision");
    assert_eq!(context.key_decisions[0].description, "Use SurrealDB");
    assert_eq!(
        context.key_decisions[0].confidence, 0.9,
        "Important = high confidence"
    );
    assert_eq!(context.key_decisions[0].alternatives.len(), 2);

    // Should have 1 question
    assert_eq!(context.open_questions.len(), 1, "Should have 1 question");
    assert_eq!(context.open_questions[0].question, "How to normalize?");

    // Should have conversation flow
    assert_eq!(
        context.conversation_flow.len(),
        2,
        "Should have 2 flow items"
    );

    // Should have key concepts from entities
    assert_eq!(context.key_concepts.len(), 1, "Should have 1 concept");
    assert_eq!(context.key_concepts[0].name, "SurrealDB");
}

#[tokio::test]
async fn test_full_normalized_roundtrip() {
    use post_cortex_core::core::context_update::CodeReference as CoreCodeRef;

    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .expect("Failed to create SurrealDB storage");

    let session_id = Uuid::new_v4();

    // Create entity graph with entity
    let mut entity_graph = SimpleEntityGraph::new();
    entity_graph.add_or_update_entity(
        "TestEntity".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Test description",
    );

    // Create context update with code reference
    let update = ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: UpdateType::CodeChanged,
        content: UpdateContent {
            title: "Implement feature".to_string(),
            description: "Added new functionality".to_string(),
            details: vec!["Detail 1".to_string()],
            examples: vec![],
            implications: vec![],
        },
        related_code: Some(CoreCodeRef {
            file_path: "src/lib.rs".to_string(),
            start_line: 100,
            end_line: 150,
            code_snippet: "pub fn new_feature() {}".to_string(),
            commit_hash: Some("abc123".to_string()),
            branch: Some("main".to_string()),
            change_description: "New feature implementation".to_string(),
        }),
        parent_update: None,
        user_marked_important: true,
        creates_entities: vec!["TestEntity".to_string()],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    };

    // Create session with update and entity graph
    let session = create_test_session_with_updates(
        session_id,
        "Full Roundtrip Test",
        vec![update],
        entity_graph,
    );

    // Save
    storage
        .save_session(&session)
        .await
        .expect("Failed to save session");

    // Load
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session");

    // Verify all data is preserved
    assert_eq!(
        loaded.name().clone(),
        Some("Full Roundtrip Test".to_string())
    );
    assert_eq!(loaded.hot_context.len(), 1);

    // Verify entities
    let entities = loaded.entity_graph.get_all_entities();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].name, "TestEntity");

    // Verify code_references extracted
    let code_refs = &loaded.code_references;
    assert_eq!(code_refs.len(), 1);
    assert!(code_refs.contains_key("src/lib.rs"));

    // Verify change_history extracted
    assert_eq!(loaded.change_history.len(), 1);
    assert_eq!(
        loaded.change_history[0].description,
        "Added new functionality"
    );
}

/// register_source upserts on entry_id: it must create the reference on a
/// table that already has other rows, keep a single row on re-registration,
/// and pick up the new hash. Guards the `UPSERT … WHERE` path across
/// SurrealDB upgrades (3.3 evaluates WHERE first).
#[tokio::test]
async fn test_register_source_creates_then_updates_single_row() {
    use crate::traits::FreshnessStorage;
    use post_cortex_proto::pb::{FreshnessStatus, SourceReference};

    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .expect("in-memory storage");
    let session = Uuid::new_v4();
    let entry_id = "src/lib.rs::parse/v1".to_string();
    let reference = |hash: &[u8]| SourceReference {
        entry_id: entry_id.clone(),
        file_path: "src/lib.rs".into(),
        content_hash: hash.to_vec(),
        captured_at_unix: 1,
        scope: None,
    };

    // An unrelated row first, so the WHERE has rows to scan and none matches.
    let mut other = reference(b"x");
    other.entry_id = "src/other.rs::f/v1".into();
    storage.register_source(session, other).await.unwrap();

    storage
        .register_source(session, reference(b"h1"))
        .await
        .unwrap();
    let first = storage.check_freshness(&entry_id, b"h1").await.unwrap();
    assert_eq!(
        first.status,
        FreshnessStatus::Fresh as i32,
        "created on first register"
    );

    storage
        .register_source(session, reference(b"h2"))
        .await
        .unwrap();
    let second = storage.check_freshness(&entry_id, b"h2").await.unwrap();
    assert_eq!(
        second.status,
        FreshnessStatus::Fresh as i32,
        "hash updated on re-register"
    );

    let mut res = storage
        .db
        .query("SELECT count() AS n FROM source_reference GROUP ALL")
        .await
        .unwrap();
    let n: Option<i64> = res.take("n").unwrap();
    assert_eq!(n, Some(2), "re-registering must not duplicate the row");
}

/// Sessions written with `name`/`description` = NULL (instead of NONE) by
/// older clients must still load; three real sessions were unloadable
/// ("Session not found") because of it.
#[tokio::test]
async fn test_load_session_with_null_name_and_description() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .expect("in-memory storage");
    let id = Uuid::new_v4();
    storage
        .db
        .query(
            "CREATE type::record('session', $id) CONTENT { session_id: $id, name: NULL, \
             description: NULL, created_at: '2026-05-17T16:34:52Z', last_updated: '2026-05-17T16:59:20Z', \
             user_preferences: {}, vectorized_update_ids: [], total_updates: 0 }",
        )
        .bind(("id", id.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();

    let session = storage
        .load_session(id)
        .await
        .expect("NULL name must not hide the session");
    assert_eq!(session.id(), id);
    assert!(session.name().is_none());
}

fn graph_entity(name: &str) -> EntityData {
    EntityData {
        aliases: Vec::new(),
        name: name.to_string(),
        entity_type: EntityType::Concept,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 2,
        importance_score: 0.5,
        description: None,
    }
}

fn graph_rel(from: &str, to: &str, relation_type: RelationType) -> EntityRelationship {
    EntityRelationship {
        from_entity: from.to_string(),
        to_entity: to.to_string(),
        relation_type,
        context: format!("{from} -> {to}"),
    }
}

/// Graph traversal over entity names that need escaping inside a SurrealQL
/// record id (`` ` ``, quotes, `;`): they must be bound, not interpolated.
#[tokio::test]
async fn test_graph_traversal_with_hostile_entity_names() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let sid = Uuid::new_v4();
    let a = "a` ; DELETE entity; --";
    let b = "b'\"name";
    let c = "c\\path";
    for name in [a, b, c, "lonely"] {
        storage
            .upsert_entity(sid, &graph_entity(name))
            .await
            .unwrap();
    }
    storage
        .create_relationship(sid, &graph_rel(a, b, RelationType::DependsOn))
        .await
        .unwrap();
    storage
        .create_relationship(sid, &graph_rel(b, c, RelationType::LeadsTo))
        .await
        .unwrap();

    let names: Vec<String> = storage
        .list_entities(sid)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names.len(), 4, "entities: {names:?}");
    assert!(storage.get_entity(sid, a).await.unwrap().is_some());

    let mut related = storage.find_related_entities(sid, b).await.unwrap();
    related.sort();
    let mut expected = vec![a.to_string(), c.to_string()];
    expected.sort();
    assert_eq!(related, expected);
    assert_eq!(
        storage
            .find_related_by_type(sid, b, &RelationType::LeadsTo)
            .await
            .unwrap(),
        vec![c.to_string()]
    );
    let network = storage.get_entity_network(sid, b, 2).await.unwrap();
    assert!(network.entities.contains_key(a) && network.entities.contains_key(c));
    assert_eq!(network.relationships.len(), 2);

    assert_eq!(storage.load_all_relationships(sid).await.unwrap().len(), 2);

    storage.delete_entity(sid, b).await.unwrap();
    assert!(storage.get_entity(sid, b).await.unwrap().is_none());
    assert!(
        storage
            .load_all_relationships(sid)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(storage.list_entities(sid).await.unwrap().len(), 3);
}

/// Same traversals with plain names, to pin the pre-existing behaviour.
#[tokio::test]
async fn test_graph_traversal_plain_names() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let sid = Uuid::new_v4();
    for name in ["A", "B", "C", "D"] {
        storage
            .upsert_entity(sid, &graph_entity(name))
            .await
            .unwrap();
    }
    storage
        .create_relationship(sid, &graph_rel("A", "B", RelationType::DependsOn))
        .await
        .unwrap();
    storage
        .create_relationship(sid, &graph_rel("B", "C", RelationType::LeadsTo))
        .await
        .unwrap();

    let mut related = storage.find_related_entities(sid, "B").await.unwrap();
    related.sort();
    assert_eq!(related, vec!["A", "C"]);
    assert_eq!(
        storage
            .find_related_by_type(sid, "B", &RelationType::LeadsTo)
            .await
            .unwrap(),
        vec!["C"]
    );
    let path = storage.find_shortest_path(sid, "A", "C").await.unwrap();
    assert_eq!(
        path,
        Some(vec!["A".to_string(), "B".to_string(), "C".to_string()])
    );
    assert_eq!(
        storage.find_shortest_path(sid, "A", "D").await.unwrap(),
        None
    );
    let network = storage.get_entity_network(sid, "B", 2).await.unwrap();
    assert!(network.entities.contains_key("A") && network.entities.contains_key("C"));
    assert_eq!(storage.load_all_relationships(sid).await.unwrap().len(), 2);
    storage.delete_entity(sid, "B").await.unwrap();
    assert!(
        storage
            .load_all_relationships(sid)
            .await
            .unwrap()
            .is_empty()
    );
}
