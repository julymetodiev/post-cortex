// Copyright (c) 2026 Julius ML
// MIT License

//! Entity resolution, record links and record validity survive a save and
//! a load on both backends.

use crate::test_support::Backend;
use chrono::{Duration, Utc};
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity, UpdateContent,
    UpdateMeta, UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::storage::SessionChanges;

fn update(minutes: i64, title: &str, entities: &[&str]) -> ContextUpdate {
    ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: Utc::now() - Duration::hours(1) + Duration::minutes(minutes),
        update_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: title.to_string(),
            description: format!("about {title}"),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: entities.iter().map(|e| e.to_string()).collect(),
        creates_relationships: match entities {
            [a, b, ..] => vec![EntityRelationship {
                from_entity: a.to_string(),
                to_entity: b.to_string(),
                relation_type: RelationType::DependsOn,
                context: "uses".into(),
            }],
            _ => vec![],
        },
        references_entities: vec![],
        typed_entities: entities
            .iter()
            .map(|e| TypedEntity {
                name: e.to_string(),
                entity_type: EntityType::Technology,
            })
            .collect(),
        author: Some("claude-code".into()),
        derived_from: Vec::new(),
        meta: UpdateMeta::default(),
    }
}

async fn session(records: &[ContextUpdate]) -> ActiveSession {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    for record in records {
        session.apply_record(record.clone());
    }
    session
}

/// Aliases, explicit merges, record links and `meta` come back after a
/// reload; a stored graph with two spellings of one entity (from before
/// resolution) loads as one entity.
async fn graph_survives_reload(storage: &impl Backend) {
    let first = update(0, "storage", &["pcx", "SurrealDB"]);
    let second = update(5, "schema", &["pcx", "surreal-db"]);
    let mut third = update(9, "replace", &["Surreal"]);
    third.meta = UpdateMeta {
        valid_from: Some(first.timestamp + Duration::minutes(2)),
        supersedes: vec![first.id],
        confidence: Some(0.75),
        retired_at: None,
    };
    let mut s = session(&[first.clone(), second.clone(), third.clone()]).await;
    assert!(s.entity_graph_mut().merge_entities("Surreal", "SurrealDB"));
    storage
        .save_session(&s, SessionChanges::everything(&s, &[]))
        .await
        .unwrap();

    let loaded = storage.load_session(s.id()).await.unwrap().session;
    let graph = &loaded.entity_graph();
    let surreal = &graph.entities["SurrealDB"];
    let mut aliases = surreal.aliases.clone();
    aliases.sort();
    assert_eq!(aliases, ["Surreal", "surreal-db"]);
    assert_eq!(graph.resolve("surreal db"), Some("SurrealDB"));
    assert_eq!(graph.resolve("Surreal"), Some("SurrealDB"));
    assert!(!graph.entities.contains_key("Surreal"));
    assert_eq!(
        graph.entity_mentions["SurrealDB"].to_vec(),
        [first.id, second.id, third.id],
        "links are rebuilt from the records"
    );
    assert_eq!(graph.entity_mentions["pcx"].to_vec(), [first.id, second.id]);
    assert_eq!(graph.edge_count(), 1);

    let stored = loaded.records().iter().find(|u| u.id == third.id).unwrap();
    assert_eq!(stored.meta, third.meta);

    // Saving the loaded copy and loading again does not drift.
    storage
        .save_session(&loaded, SessionChanges::everything(&loaded, &[]))
        .await
        .unwrap();
    let again = storage.load_session(s.id()).await.unwrap().session;
    assert_eq!(again.entity_graph().entities.len(), 2);
    assert_eq!(
        again.entity_graph().entities["SurrealDB"].mention_count,
        surreal.mention_count
    );
}

/// A retired record is stored with its retirement; a purge-style rebuild
/// saved as a whole graph leaves no trace of the removed record.
async fn forget_and_purge_persist(storage: &impl Backend) {
    let keep = update(0, "keep", &["api", "core"]);
    let secret = update(3, "secret", &["api", "leaked-host"]);
    let mut s = session(&[keep.clone(), secret.clone()]).await;
    storage
        .save_session(&s, SessionChanges::everything(&s, &[]))
        .await
        .unwrap();

    let retired = s.retire_update(&keep.id, Utc::now()).unwrap();
    assert!(s.retire_update(&keep.id, Utc::now()).is_none(), "once");
    storage
        .save_session(
            &s,
            SessionChanges {
                updates: vec![retired.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let loaded = storage.load_session(s.id()).await.unwrap().session;
    let stored = loaded.records().iter().find(|u| u.id == keep.id).unwrap();
    assert_eq!(stored.meta.retired_at, retired.meta.retired_at);

    assert!(s.remove_records(&[secret.id]).len() == 1);
    s.rebuild_entity_graph();
    storage
        .save_session(
            &s,
            SessionChanges {
                removed: vec![secret.id],
                whole_graph: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let purged = storage.load_session(s.id()).await.unwrap().session;
    assert!(!purged.entity_graph().entities.contains_key("leaked-host"));
    assert!(purged.entity_graph().entities.contains_key("api"));
    assert_eq!(purged.entity_graph().edge_count(), 1);
    assert!(purged.records().iter().all(|u| u.id != secret.id));
}

#[tokio::test]
async fn rocksdb_graph() {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::RealRocksDBStorage::new(dir.path()).await.unwrap();
    graph_survives_reload(&storage).await;
    forget_and_purge_persist(&storage).await;
}

#[cfg(feature = "surrealdb-storage")]
#[tokio::test]
async fn surrealdb_graph() {
    let storage = crate::SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    graph_survives_reload(&storage).await;
    forget_and_purge_persist(&storage).await;
}
