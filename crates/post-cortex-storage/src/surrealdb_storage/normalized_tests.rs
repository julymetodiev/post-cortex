// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Sessions stored in normalized tables (records, entities, relations) load
//! back whole.

use super::SurrealDBStorage;
use chrono::Utc;
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::{ActiveSession, UserPreferences};
use post_cortex_core::session::session_components::SessionMetadata;
use post_cortex_ports::storage::SessionStore;
use uuid::Uuid;

/// Helper to create test session with pre-populated data
fn create_test_session_with_updates(
    session_id: Uuid,
    name: &str,
    updates: Vec<ContextUpdate>,
    entity_graph: SimpleEntityGraph,
) -> ActiveSession {
    ActiveSession::from_records(
        SessionMetadata::new(
            session_id,
            Some(name.to_string()),
            Some("Test session".to_string()),
            UserPreferences::default(),
            Utc::now(),
        ),
        Utc::now(),
        updates,
        entity_graph,
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
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .expect("Failed to save session");

    // Verify updates are in context_update table
    let loaded_updates =
        crate::surrealdb_storage::sessions::load_session_updates(&storage.db(), session_id)
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
        .expect("Failed to load session")
        .session;

    // Verify hot_context was rebuilt
    let hot_updates: Vec<_> = loaded.hot_records().collect();
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
    entity_graph.add_relationship(relationship, chrono::Utc::now());

    // Create session with entity graph
    let session =
        create_test_session_with_updates(session_id, "Graph Test", Vec::new(), entity_graph);

    // Save session
    storage
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .expect("Failed to save session");

    // Verify entities in native table
    let entities = crate::surrealdb_storage::graph::load_entities(&storage.db(), session_id)
        .await
        .expect("Failed to list entities");
    assert_eq!(entities.len(), 2, "Should have 2 entities");

    let rust_entity = entities.iter().find(|e| e.name == "Rust");
    assert!(rust_entity.is_some(), "Should find Rust entity");
    assert_eq!(rust_entity.unwrap().entity_type, EntityType::Technology);

    // Verify relationships via load_all_relationships
    let relationships =
        crate::surrealdb_storage::graph::load_all_relationships(&storage.db(), session_id)
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
        .expect("Failed to load session")
        .session;

    let loaded_entities = loaded.entity_graph().get_all_entities();
    assert_eq!(
        loaded_entities.len(),
        2,
        "Loaded session should have 2 entities"
    );

    let loaded_relationships = loaded.entity_graph().get_all_relationships();
    assert_eq!(
        loaded_relationships.len(),
        1,
        "Loaded session should have 1 relationship"
    );
    assert_eq!(loaded_relationships[0].context, "Rust client for SurrealDB");
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
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .expect("Failed to save session");

    // Load
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session")
        .session;

    // Verify all data is preserved
    assert_eq!(
        loaded.name().clone(),
        Some("Full Roundtrip Test".to_string())
    );
    assert_eq!(loaded.hot_records().len(), 1);

    // Verify entities
    let entities = loaded.entity_graph().get_all_entities();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].name, "TestEntity");

    // Verify code_references extracted
    let code_refs = &loaded.code_references();
    assert_eq!(code_refs.len(), 1);
    assert!(code_refs.contains_key("src/lib.rs"));

    // Verify change_history extracted
    assert_eq!(loaded.change_history().len(), 1);
    assert_eq!(
        loaded.change_history()[0].description,
        "Added new functionality"
    );
}
