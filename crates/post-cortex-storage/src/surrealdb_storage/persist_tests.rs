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

//! Incremental session persistence, bound record ids, single-query
//! listings. Run against the embedded engine by default; set
//! `PCX_STO_TEST_SURREAL_URL` (+ `_USER`, `_PASS`, `_NS`) to run them against
//! a server, each test in its own throwaway database.

use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

use super::SurrealDBStorage;
use crate::traits::{GraphStorage, Storage, VectorStorage};
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityType, UpdateContent, UpdateType,
};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::VectorMetadata;

/// Embedded engine, or a throwaway database on the server named by
/// `PCX_STO_TEST_SURREAL_URL`. The namespace must start with `pcx_sto_test`
/// so a misconfigured run can never touch real data.
pub(super) async fn test_storage() -> SurrealDBStorage {
    let Ok(url) = std::env::var("PCX_STO_TEST_SURREAL_URL") else {
        return SurrealDBStorage::new_with_dimension("mem://", None, None, None, None, 8)
            .await
            .unwrap();
    };
    let ns = std::env::var("PCX_STO_TEST_SURREAL_NS").expect("PCX_STO_TEST_SURREAL_NS");
    assert!(
        ns.starts_with("pcx_sto_test"),
        "refusing to run tests in namespace {ns}"
    );
    let user = std::env::var("PCX_STO_TEST_SURREAL_USER").ok();
    let pass = std::env::var("PCX_STO_TEST_SURREAL_PASS").ok();
    let db = format!("t_{}", Uuid::new_v4().simple());
    SurrealDBStorage::new_with_dimension(
        &url,
        user.as_deref(),
        pass.as_deref(),
        Some(&ns),
        Some(&db),
        8,
    )
    .await
    .unwrap()
}

fn plain_update(title: &str, offset_ms: i64) -> ContextUpdate {
    ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: Utc::now() + chrono::Duration::milliseconds(offset_ms),
        update_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: title.to_string(),
            description: String::new(),
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
    }
}

fn update_titles(session: &ActiveSession) -> Vec<String> {
    session
        .incremental_updates
        .iter()
        .map(|u| u.content.title.clone())
        .collect()
}

async fn count_where(storage: &SurrealDBStorage, table: &str, session_id: Uuid) -> i64 {
    let mut res = storage
        .db
        .query(format!(
            "SELECT count() AS n FROM {table} WHERE session_id = $s GROUP ALL"
        ))
        .bind(("s", session_id.to_string()))
        .await
        .unwrap();
    let n: Option<i64> = res.take("n").unwrap();
    n.unwrap_or(0)
}

/// Every persist path leaves a row for every update; updates removed from a
/// loaded session are deleted (S10); a snapshot this process never loaded
/// cannot delete rows it does not know about.
#[tokio::test]
async fn incremental_persist_writes_missing_rows_and_deletes_removed() {
    let storage = test_storage().await;
    let id = Uuid::new_v4();
    let (u1, u2, u3) = (
        plain_update("u1", 0),
        plain_update("u2", 1),
        plain_update("u3", 2),
    );

    let mut session = ActiveSession::new(id, Some("s".into()), None);
    session.set_incremental_updates(vec![u1.clone()]);
    storage.save_session(&session).await.unwrap();

    // u2 only in the snapshot, u3 passed explicitly: both get rows.
    session.set_incremental_updates(vec![u1.clone(), u2.clone(), u3.clone()]);
    storage
        .save_session_with_updates(&session, id, vec![u3.clone()])
        .await
        .unwrap();
    assert_eq!(count_where(&storage, "context_update", id).await, 3);

    let mut loaded = storage.load_session(id).await.unwrap();
    assert_eq!(update_titles(&loaded), vec!["u1", "u2", "u3"]);
    assert!(loaded.remove_update_by_id(&u2.id));
    storage
        .save_session_with_updates(&loaded, id, vec![])
        .await
        .unwrap();
    assert_eq!(count_where(&storage, "context_update", id).await, 2);
    assert_eq!(
        update_titles(&storage.load_session(id).await.unwrap()),
        vec!["u1", "u3"]
    );

    // Another process saving an empty snapshot over the session must not
    // wipe rows it never saw.
    let other_process = SurrealDBStorage {
        persisted: Default::default(),
        ..storage.clone()
    };
    other_process
        .save_session(&ActiveSession::new(id, Some("s".into()), None))
        .await
        .unwrap();
    assert_eq!(count_where(&storage, "context_update", id).await, 2);
}

/// The incremental path only sends changed entities, but new and changed
/// ones do reach the database.
#[tokio::test]
async fn incremental_persist_writes_changed_graph() {
    let storage = test_storage().await;
    let id = Uuid::new_v4();
    let mut session = ActiveSession::new(id, None, None);
    let mut graph = SimpleEntityGraph::new();
    graph.add_or_update_entity("A".into(), EntityType::Concept, Utc::now(), "a");
    session.entity_graph = std::sync::Arc::new(graph.clone());
    storage.save_session(&session).await.unwrap();

    graph.add_or_update_entity("B".into(), EntityType::Technology, Utc::now(), "b");
    graph.add_relationship(post_cortex_core::core::context_update::EntityRelationship {
        from_entity: "A".into(),
        to_entity: "B".into(),
        relation_type: post_cortex_core::core::context_update::RelationType::DependsOn,
        context: "ctx".into(),
    });
    session.entity_graph = std::sync::Arc::new(graph);
    storage
        .save_session_with_updates(&session, id, vec![])
        .await
        .unwrap();

    assert_eq!(storage.list_entities(id).await.unwrap().len(), 2);
    assert_eq!(
        storage.find_related_entities(id, "A").await.unwrap(),
        vec!["B"]
    );

    // After a graph reset the next incremental save rewrites everything.
    storage.clear_session_entities(id).await.unwrap();
    storage
        .save_session_with_updates(&session, id, vec![])
        .await
        .unwrap();
    assert_eq!(storage.list_entities(id).await.unwrap().len(), 2);
}

/// A save/load round trip keeps every entity field, including the ones a
/// rebuild through `add_or_update_entity` would reset.
#[tokio::test]
async fn load_keeps_entity_mention_count_and_importance() {
    let storage = test_storage().await;
    let id = Uuid::new_v4();
    let first = Utc::now() - chrono::Duration::days(3);
    let last = Utc::now() - chrono::Duration::hours(1);

    let mut graph = SimpleEntityGraph::new();
    graph.insert_entity(post_cortex_core::core::context_update::EntityData {
        name: "Rust".into(),
        entity_type: EntityType::Technology,
        first_mentioned: first,
        last_mentioned: last,
        mention_count: 7,
        importance_score: 2.5,
        description: None,
    });
    graph.add_or_update_entity("Tokio".into(), EntityType::Technology, last, "runtime");
    graph.add_or_update_entity("Tokio".into(), EntityType::Technology, last, "");
    let mut session = ActiveSession::new(id, None, None);
    session.entity_graph = std::sync::Arc::new(graph);
    storage.save_session(&session).await.unwrap();

    let loaded = storage.load_session(id).await.unwrap();
    let rust = &loaded.entity_graph.entities["Rust"];
    assert_eq!(rust.mention_count, 7);
    assert_eq!(rust.importance_score, 2.5);
    assert_eq!(rust.first_mentioned.timestamp(), first.timestamp());
    assert_eq!(rust.last_mentioned.timestamp(), last.timestamp());
    assert_eq!(rust.description, None);
    let tokio = &loaded.entity_graph.entities["Tokio"];
    assert_eq!(tokio.mention_count, 2);
    assert_eq!(tokio.description.as_deref(), Some("runtime"));

    // Loading again (and saving the loaded copy) does not drift.
    storage.save_session(&loaded).await.unwrap();
    let again = storage.load_session(id).await.unwrap();
    assert_eq!(again.entity_graph.entities["Rust"].mention_count, 7);
    assert_eq!(again.entity_graph.entities["Tokio"].mention_count, 2);
}

/// Records written with the old interpolated ids (`table:`id``) are the
/// records the bound `type::record` ids address.
#[tokio::test]
async fn bound_ids_address_legacy_backtick_records() {
    let storage = test_storage().await;
    let id = Uuid::new_v4();
    let u1 = plain_update("u1", 0);
    let entity_id = SurrealDBStorage::entity_id(id, "Rust");
    storage
        .db
        .query(format!(
            "UPSERT context_update:`{}` CONTENT {{ update_id: '{}', session_id: '{}', \
             timestamp: '{}', update_type: 'ConceptDefined', update_data: {} }}; \
             UPSERT entity:`{}` CONTENT {{ session_id: '{}', name: 'Rust', \
             entity_type: 'Technology', first_mentioned: '{}', last_mentioned: '{}', \
             mention_count: 1, importance_score: 0.5 }};",
            u1.id,
            u1.id,
            id,
            u1.timestamp.to_rfc3339(),
            serde_json::to_string(&u1).unwrap(),
            entity_id,
            id,
            u1.timestamp.to_rfc3339(),
            u1.timestamp.to_rfc3339(),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

    let mut session = ActiveSession::new(id, None, None);
    session.set_incremental_updates(vec![u1.clone()]);
    let mut graph = SimpleEntityGraph::new();
    graph.add_or_update_entity("Rust".into(), EntityType::Technology, Utc::now(), "");
    session.entity_graph = std::sync::Arc::new(graph);
    storage.save_session(&session).await.unwrap();

    assert_eq!(count_where(&storage, "context_update", id).await, 1);
    assert_eq!(count_where(&storage, "entity", id).await, 1);
}

#[tokio::test]
async fn summaries_workspaces_and_delete_session() {
    let storage = test_storage().await;
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut sa = ActiveSession::new(a, Some("alpha".into()), Some("first".into()));
    sa.set_incremental_updates((0..3).map(|i| plain_update(&format!("a{i}"), i)).collect());
    storage.save_session(&sa).await.unwrap();
    storage
        .save_session(&ActiveSession::new(b, None, None))
        .await
        .unwrap();

    let summaries = storage.list_session_summaries().await.unwrap();
    assert_eq!(summaries.len(), 2);
    let alpha = summaries.iter().find(|s| s.id == a).unwrap();
    assert_eq!(alpha.name.as_deref(), Some("alpha"));
    assert_eq!(alpha.update_count, 3);
    let beta = summaries.iter().find(|s| s.id == b).unwrap();
    assert_eq!(beta.name, None);
    assert_eq!(beta.update_count, 0);

    let ws = Uuid::new_v4();
    storage
        .save_workspace_metadata(ws, "ws", "d", &[a, b])
        .await
        .unwrap();
    let workspaces = storage.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].sessions.len(), 2);

    storage
        .add_vector(
            vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            VectorMetadata {
                id: "entity:x".into(),
                text: "x".into(),
                source: a.to_string(),
                content_type: "entity".into(),
                timestamp: Utc::now(),
                metadata: HashMap::new(),
            },
        )
        .await
        .unwrap();

    storage.delete_session(a).await.unwrap();
    assert_eq!(count_where(&storage, "context_update", a).await, 0);
    assert_eq!(count_where(&storage, "embedding", a).await, 0);
    assert_eq!(
        storage.list_workspaces().await.unwrap()[0].sessions.len(),
        1
    );
    let err = storage.load_session(a).await.unwrap_err();
    assert!(crate::is_not_found(&err));
}

fn vector_meta(content_id: &str, session: &str) -> VectorMetadata {
    VectorMetadata {
        id: content_id.to_string(),
        text: format!("{content_id} in {session}"),
        source: session.to_string(),
        content_type: "entity".into(),
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    }
}

/// `entity:<name>` exists in many sessions: their vectors must not
/// overwrite each other, legacy records (keyed by the bare content id) are
/// replaced on rewrite, and removal can target one session.
#[tokio::test]
async fn embeddings_are_session_scoped() {
    let storage = test_storage().await;
    let (s1, s2) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let v = vec![0.5f32; 8];

    // Legacy record of s1, written under the old id scheme.
    storage
        .db
        .query("CREATE type::record('embedding', 'entity:rust') CONTENT $r")
        .bind((
            "r",
            super::vectors::embedding_record(v.clone(), vector_meta("entity:rust", &s1)),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

    storage
        .add_vectors_batch(vec![
            (v.clone(), vector_meta("entity:rust", &s1)),
            (v.clone(), vector_meta("entity:rust", &s2)),
        ])
        .await
        .unwrap();
    assert_eq!(storage.count_session_embeddings(&s1).await, 1);
    assert_eq!(storage.count_session_embeddings(&s2).await, 1);
    assert_eq!(storage.total_count().await, 2);

    assert!(
        storage
            .remove_session_vector(&s1, "entity:rust")
            .await
            .unwrap()
    );
    assert!(
        !storage
            .remove_session_vector(&s1, "entity:rust")
            .await
            .unwrap()
    );
    assert_eq!(storage.count_session_embeddings(&s2).await, 1);
    assert!(storage.remove_vector("entity:rust").await.unwrap());
    assert_eq!(storage.total_count().await, 0);
}
