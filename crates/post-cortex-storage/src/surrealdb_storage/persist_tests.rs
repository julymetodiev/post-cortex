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

//! Session saves by their changes, bound record ids, single-query
//! listings. Run against the embedded engine by default; set
//! `PCX_STO_TEST_SURREAL_URL` (+ `_USER`, `_PASS`, `_NS`) to run them against
//! a server, each test in its own throwaway database.

use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

use super::SurrealDBStorage;
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityType, UpdateContent, UpdateType,
};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::VectorMetadata;
use post_cortex_ports::storage::{SessionChanges, SessionStore, VectorStore, WorkspaceStore};

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
        meta: Default::default(),
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
        author: None,
        derived_from: Vec::new(),
    }
}

/// Every record, the whole graph.
fn everything(session: &ActiveSession) -> SessionChanges {
    SessionChanges::everything(session, &[])
}

/// The whole graph, nothing else.
fn whole_graph() -> SessionChanges {
    SessionChanges {
        whole_graph: true,
        ..Default::default()
    }
}

fn update_titles(session: &ActiveSession) -> Vec<String> {
    session
        .records()
        .iter()
        .map(|u| u.content.title.clone())
        .collect()
}

async fn count_where(storage: &SurrealDBStorage, table: &str, session_id: Uuid) -> i64 {
    let mut res = storage
        .db()
        .query(format!(
            "SELECT count() AS n FROM {table} WHERE session_id = $s GROUP ALL"
        ))
        .bind(("s", session_id.to_string()))
        .await
        .unwrap();
    let n: Option<i64> = res.take("n").unwrap();
    n.unwrap_or(0)
}

/// A save writes the records it names and deletes the removals it names;
/// a snapshot that lacks a record (it predates it) deletes nothing.
#[tokio::test]
async fn saves_write_and_remove_the_records_they_name() {
    let storage = test_storage().await;
    let id = Uuid::new_v4();
    let (u1, u2, u3) = (
        plain_update("u1", 0),
        plain_update("u2", 1),
        plain_update("u3", 2),
    );

    let mut session = ActiveSession::new(id, Some("s".into()), None, chrono::Utc::now());
    session.replace_records(vec![u1.clone()]);
    storage
        .save_session(&session, everything(&session))
        .await
        .unwrap();

    session.replace_records(vec![u1.clone(), u2.clone(), u3.clone()]);
    storage
        .save_session(
            &session,
            SessionChanges {
                updates: vec![u2.clone(), u3.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(count_where(&storage, "context_update", id).await, 3);

    let mut loaded = storage.load_session(id).await.unwrap().session;
    assert_eq!(update_titles(&loaded), vec!["u1", "u2", "u3"]);
    assert!(loaded.remove_records(&[u2.id]).len() == 1);
    // Lacking u2 is not enough: deletions are named.
    storage
        .save_session(&loaded, SessionChanges::default())
        .await
        .unwrap();
    assert_eq!(count_where(&storage, "context_update", id).await, 3);
    storage
        .save_session(
            &loaded,
            SessionChanges {
                removed: vec![u2.id],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(count_where(&storage, "context_update", id).await, 2);
    assert_eq!(
        update_titles(&storage.load_session(id).await.unwrap().session),
        vec!["u1", "u3"]
    );

    // An empty snapshot saved over the session wipes nothing.
    storage
        .save_session(
            &ActiveSession::new(id, Some("s".into()), None, chrono::Utc::now()),
            SessionChanges::default(),
        )
        .await
        .unwrap();
    assert_eq!(count_where(&storage, "context_update", id).await, 2);
}

/// A save writes the entities and edges its records name; a whole-graph
/// save replaces the stored graph, its lost rows included.
#[tokio::test]
async fn saves_write_the_graph_rows_they_change() {
    use post_cortex_core::core::context_update::{EntityRelationship, RelationType, TypedEntity};
    let storage = test_storage().await;
    let id = Uuid::new_v4();
    let mut session = ActiveSession::new(id, None, None, chrono::Utc::now());
    let mut first = plain_update("a", 0);
    first.typed_entities = vec![TypedEntity {
        name: "A".into(),
        entity_type: EntityType::Concept,
    }];
    session.apply_record(first);
    storage
        .save_session(&session, everything(&session))
        .await
        .unwrap();

    let mut second = plain_update("ab", 1);
    second.typed_entities = vec![TypedEntity {
        name: "B".into(),
        entity_type: EntityType::Technology,
    }];
    second.creates_relationships = vec![EntityRelationship {
        from_entity: "A".into(),
        to_entity: "B".into(),
        relation_type: RelationType::DependsOn,
        context: "ctx".into(),
    }];
    let second = ContextUpdate::clone(&session.apply_record(second));
    storage
        .save_session(
            &session,
            SessionChanges {
                updates: vec![second],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let entities = |storage: &SurrealDBStorage| {
        let db = std::sync::Arc::clone(&storage.db());
        async move {
            crate::surrealdb_storage::graph::load_entities(&db, id)
                .await
                .unwrap()
                .len()
        }
    };
    assert_eq!(entities(&storage).await, 2);
    assert_eq!(
        crate::surrealdb_storage::graph::load_all_relationships(&storage.db(), id)
            .await
            .unwrap()
            .len(),
        1
    );

    session.entity_graph_mut().remove_entity("B");
    storage.save_session(&session, whole_graph()).await.unwrap();
    assert_eq!(entities(&storage).await, 1);
    assert!(
        crate::surrealdb_storage::graph::load_all_relationships(&storage.db(), id)
            .await
            .unwrap()
            .is_empty()
    );
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
        aliases: Vec::new(),
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
    let mut session = ActiveSession::new(id, None, None, chrono::Utc::now());
    *session.entity_graph_mut() = graph;
    storage.save_session(&session, whole_graph()).await.unwrap();

    let loaded = storage.load_session(id).await.unwrap().session;
    let rust = &loaded.entity_graph().entities["Rust"];
    assert_eq!(rust.mention_count, 7);
    assert_eq!(rust.importance_score, 2.5);
    assert_eq!(rust.first_mentioned.timestamp(), first.timestamp());
    assert_eq!(rust.last_mentioned.timestamp(), last.timestamp());
    assert_eq!(rust.description, None);
    let tokio = &loaded.entity_graph().entities["Tokio"];
    assert_eq!(tokio.mention_count, 2);
    assert_eq!(tokio.description.as_deref(), Some("runtime"));

    // Loading again (and saving the loaded copy) does not drift.
    storage.save_session(&loaded, whole_graph()).await.unwrap();
    let again = storage.load_session(id).await.unwrap().session;
    assert_eq!(again.entity_graph().entities["Rust"].mention_count, 7);
    assert_eq!(again.entity_graph().entities["Tokio"].mention_count, 2);
}

/// Records written with the old interpolated ids (`table:`id``) are the
/// records the bound `type::record` ids address.
#[tokio::test]
async fn bound_ids_address_legacy_backtick_records() {
    let storage = test_storage().await;
    let id = Uuid::new_v4();
    let u1 = plain_update("u1", 0);
    let entity_id = crate::surrealdb_storage::ids::entity_id(id, "Rust");
    storage
        .db()
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

    let mut session = ActiveSession::new(id, None, None, chrono::Utc::now());
    session.replace_records(vec![u1.clone()]);
    let mut graph = SimpleEntityGraph::new();
    graph.add_or_update_entity("Rust".into(), EntityType::Technology, Utc::now(), "");
    *session.entity_graph_mut() = graph;
    storage
        .save_session(&session, everything(&session))
        .await
        .unwrap();

    assert_eq!(count_where(&storage, "context_update", id).await, 1);
    assert_eq!(count_where(&storage, "entity", id).await, 1);
}

#[tokio::test]
async fn summaries_workspaces_and_delete_session() {
    let storage = test_storage().await;
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut sa = ActiveSession::new(
        a,
        Some("alpha".into()),
        Some("first".into()),
        chrono::Utc::now(),
    );
    sa.replace_records((0..3).map(|i| plain_update(&format!("a{i}"), i)).collect());
    storage.save_session(&sa, everything(&sa)).await.unwrap();
    let sb = ActiveSession::new(b, None, None, chrono::Utc::now());
    storage.save_session(&sb, everything(&sb)).await.unwrap();

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
        .save_workspace_metadata(ws, "ws", &ws.to_string(), "d", &[a, b])
        .await
        .unwrap();
    let workspaces = storage.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].sessions.len(), 2);

    storage
        .upsert_vectors(vec![(
            vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            VectorMetadata {
                id: "entity:x".into(),
                text: "x".into(),
                source: a.to_string(),
                content_type: "entity".into(),
                timestamp: Utc::now(),
                metadata: HashMap::new(),
            },
        )])
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
    assert!(err.is_not_found());
}
