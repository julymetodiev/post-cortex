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
use uuid::Uuid;

use super::SurrealDBStorage;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType,
};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::storage::{SessionChanges, SessionStore};

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
        chrono::Utc::now(),
    );

    // Save session
    storage
        .save_session(&session, SessionChanges::everything(&session, &[]))
        .await
        .expect("Failed to save session");

    // Load session
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session")
        .session;
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

    let mut session = ActiveSession::new(session_id, None, None, Utc::now());
    session.entity_graph_mut().insert_entity(entity);
    save_whole(&storage, &session).await;

    let entities = crate::surrealdb_storage::graph::load_entities(&storage.db(), session_id)
        .await
        .expect("Failed to list entities");
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].name, "rust");

    session.entity_graph_mut().remove_entity("rust");
    save_whole(&storage, &session).await;
    let left = crate::surrealdb_storage::graph::load_entities(&storage.db(), session_id)
        .await
        .expect("Failed to check deleted");
    assert!(left.is_empty());
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
        chrono::Utc::now(),
    );

    // Save session
    storage
        .save_session(&session, SessionChanges::everything(&session, &[]))
        .await
        .expect("Failed to save session to remote");

    // Load session
    let loaded = storage
        .load_session(session_id)
        .await
        .expect("Failed to load session from remote")
        .session;
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

/// register_source upserts on entry_id: it must create the reference on a
/// table that already has other rows, keep a single row on re-registration,
/// and pick up the new hash. Guards the `UPSERT … WHERE` path across
/// SurrealDB upgrades (3.3 evaluates WHERE first).
#[tokio::test]
async fn test_register_source_creates_then_updates_single_row() {
    use post_cortex_core::core::freshness::{FreshnessStatus, SourceReference, SourceScope};
    use post_cortex_ports::storage::FreshnessStore;

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
        scope: SourceScope::WholeFile,
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
        FreshnessStatus::Fresh,
        "created on first register"
    );

    storage
        .register_source(session, reference(b"h2"))
        .await
        .unwrap();
    let second = storage.check_freshness(&entry_id, b"h2").await.unwrap();
    assert_eq!(
        second.status,
        FreshnessStatus::Fresh,
        "hash updated on re-register"
    );

    let mut res = storage
        .db()
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
        .db()
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
        .expect("NULL name must not hide the session")
        .session;
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

/// Save session `sid` with the graph of `entities` and `edges` (a
/// whole-graph save); returns the session.
async fn save_graph(
    storage: &SurrealDBStorage,
    sid: Uuid,
    entities: &[&str],
    edges: Vec<EntityRelationship>,
) -> ActiveSession {
    let mut graph = SimpleEntityGraph::new();
    for name in entities {
        graph.insert_entity(graph_entity(name));
    }
    for edge in edges {
        graph.add_relationship(edge, Utc::now());
    }
    let mut session = ActiveSession::new(sid, None, None, Utc::now());
    *session.entity_graph_mut() = graph;
    save_whole(storage, &session).await;
    session
}

async fn save_whole(storage: &SurrealDBStorage, session: &ActiveSession) {
    storage
        .save_session(
            session,
            SessionChanges {
                whole_graph: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

fn graph_rel(from: &str, to: &str, relation_type: RelationType) -> EntityRelationship {
    EntityRelationship {
        from_entity: from.to_string(),
        to_entity: to.to_string(),
        relation_type,
        context: format!("{from} -> {to}"),
    }
}

/// Entity names that need escaping inside a SurrealQL record id (`` ` ``,
/// quotes, `;`): they must be bound, not interpolated.
#[tokio::test]
async fn test_graph_storage_with_hostile_entity_names() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let sid = Uuid::new_v4();
    let a = "a` ; DELETE entity; --";
    let b = "b'\"name";
    let c = "c\\path";
    let mut session = save_graph(
        &storage,
        sid,
        &[a, b, c, "lonely"],
        vec![
            graph_rel(a, b, RelationType::DependsOn),
            graph_rel(b, c, RelationType::LeadsTo),
        ],
    )
    .await;

    let names: Vec<String> = crate::surrealdb_storage::graph::load_entities(&storage.db(), sid)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names.len(), 4, "entities: {names:?}");
    assert!(names.iter().any(|n| n == a), "{names:?}");

    let mut edges: Vec<(String, String)> =
        crate::surrealdb_storage::graph::load_all_relationships(&storage.db(), sid)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.from_entity, r.to_entity))
            .collect();
    edges.sort();
    let mut expected = vec![
        (a.to_string(), b.to_string()),
        (b.to_string(), c.to_string()),
    ];
    expected.sort();
    assert_eq!(edges, expected);

    session.entity_graph_mut().remove_entity(b);
    save_whole(&storage, &session).await;
    let names: Vec<String> = crate::surrealdb_storage::graph::load_entities(&storage.db(), sid)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(!names.iter().any(|n| n == b), "{names:?}");
    assert!(
        crate::surrealdb_storage::graph::load_all_relationships(&storage.db(), sid)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        crate::surrealdb_storage::graph::load_entities(&storage.db(), sid)
            .await
            .unwrap()
            .len(),
        3
    );
}

/// Plain names: relationships are stored, and a whole-graph save without
/// an entity removes its edges.
#[tokio::test]
async fn test_graph_storage_plain_names() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let sid = Uuid::new_v4();
    let mut session = save_graph(
        &storage,
        sid,
        &["A", "B", "C", "D"],
        vec![
            graph_rel("A", "B", RelationType::DependsOn),
            graph_rel("B", "C", RelationType::LeadsTo),
        ],
    )
    .await;

    assert_eq!(
        crate::surrealdb_storage::graph::load_all_relationships(&storage.db(), sid)
            .await
            .unwrap()
            .len(),
        2
    );
    session.entity_graph_mut().remove_entity("B");
    save_whole(&storage, &session).await;
    assert!(
        crate::surrealdb_storage::graph::load_all_relationships(&storage.db(), sid)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Two spellings stored as separate rows before resolution existed load as
/// one entity (SurrealDB keeps one row per name).
#[tokio::test]
async fn legacy_spellings_merge_on_load() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, Utc::now());
    session.apply_record(post_cortex_ports::conformance::record(0, "a", &["Rust"]));
    let sid = session.id();
    storage
        .save_session(&session, SessionChanges::everything(&session, &[]))
        .await
        .unwrap();
    let older = post_cortex_ports::conformance::at(-30 * 24 * 3600);
    let legacy = EntityData {
        name: "rust".into(),
        entity_type: EntityType::Technology,
        first_mentioned: older,
        last_mentioned: older,
        mention_count: 4,
        importance_score: 1.0,
        description: None,
        aliases: Vec::new(),
    };
    storage
        .db()
        .query("UPSERT type::record('entity', $key) CONTENT $row;")
        .bind(("key", crate::surrealdb_storage::ids::entity_id(sid, "rust")))
        .bind((
            "row",
            crate::surrealdb_storage::graph::entity_record(sid, &legacy),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
    let loaded = storage.load_session(sid).await.unwrap().session;
    let graph = &loaded.entity_graph();
    assert_eq!(graph.entities.len(), 1);
    let rust = &graph.entities["rust"];
    assert_eq!(rust.aliases, ["Rust"], "the earlier spelling wins");
    assert_eq!(rust.mention_count, 5);
    assert_eq!(graph.resolve("Rust"), Some("rust"));
}
