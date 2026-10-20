// Copyright (c) 2026 Julius ML
// MIT License

//! Entity resolution, record links and record validity survive a save and
//! a load on both backends (ROADMAP Phase 3 items 2, 7, 9; Phase 4).

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::traits::Storage;
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity, UpdateContent,
    UpdateMeta, UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;

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
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None);
    for record in records {
        session
            .add_incremental_update(record.clone())
            .await
            .unwrap();
    }
    session
}

/// Aliases, explicit merges, record links and `meta` come back after a
/// reload; a stored graph with two spellings of one entity (from before
/// resolution) loads as one entity.
async fn graph_survives_reload(storage: &impl Storage) {
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
    assert!(std::sync::Arc::make_mut(&mut s.entity_graph).merge_entities("Surreal", "SurrealDB"));
    storage.save_session(&s).await.unwrap();

    let loaded = storage.load_session(s.id()).await.unwrap();
    let graph = &loaded.entity_graph;
    let surreal = &graph.entities["SurrealDB"];
    let mut aliases = surreal.aliases.clone();
    aliases.sort();
    assert_eq!(aliases, ["Surreal", "surreal-db"]);
    assert_eq!(graph.resolve("surreal db"), Some("SurrealDB"));
    assert_eq!(graph.resolve("Surreal"), Some("SurrealDB"));
    assert!(!graph.entities.contains_key("Surreal"));
    assert_eq!(
        graph.entity_mentions["SurrealDB"],
        [first.id, second.id, third.id],
        "links are rebuilt from the records"
    );
    assert_eq!(graph.entity_mentions["pcx"], [first.id, second.id]);
    assert_eq!(graph.edge_count(), 1);

    let stored = loaded
        .incremental_updates
        .iter()
        .find(|u| u.id == third.id)
        .unwrap();
    assert_eq!(stored.meta, third.meta);

    // Saving the loaded copy and loading again does not drift.
    storage.save_session(&loaded).await.unwrap();
    let again = storage.load_session(s.id()).await.unwrap();
    assert_eq!(again.entity_graph.entities.len(), 2);
    assert_eq!(
        again.entity_graph.entities["SurrealDB"].mention_count,
        surreal.mention_count
    );
}

/// A retired record is stored with its retirement; a purge-style rebuild
/// with the stored graph cleared leaves no trace of the removed record.
async fn forget_and_purge_persist(storage: &impl Storage) {
    let keep = update(0, "keep", &["api", "core"]);
    let secret = update(3, "secret", &["api", "leaked-host"]);
    let mut s = session(&[keep.clone(), secret.clone()]).await;
    storage.save_session(&s).await.unwrap();

    let retired = s.retire_update(&keep.id, Utc::now()).unwrap();
    assert!(s.retire_update(&keep.id, Utc::now()).is_none(), "once");
    storage
        .save_session_with_updates(&s, s.id(), vec![retired.clone()])
        .await
        .unwrap();
    let loaded = storage.load_session(s.id()).await.unwrap();
    let stored = loaded
        .incremental_updates
        .iter()
        .find(|u| u.id == keep.id)
        .unwrap();
    assert_eq!(stored.meta.retired_at, retired.meta.retired_at);

    assert!(s.remove_update_by_id(&secret.id));
    s.rebuild_entity_graph_sync();
    storage.clear_session_entities(s.id()).await.unwrap();
    storage
        .save_session_with_changes(&s, s.id(), vec![], vec![secret.id])
        .await
        .unwrap();
    let purged = storage.load_session(s.id()).await.unwrap();
    assert!(!purged.entity_graph.entities.contains_key("leaked-host"));
    assert!(purged.entity_graph.entities.contains_key("api"));
    assert_eq!(purged.entity_graph.edge_count(), 1);
    assert!(purged.incremental_updates.iter().all(|u| u.id != secret.id));
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

/// Two spellings stored as separate rows before resolution existed load as
/// one entity (SurrealDB keeps one row per name).
#[cfg(feature = "surrealdb-storage")]
#[tokio::test]
async fn surrealdb_legacy_spellings_merge_on_load() {
    use crate::traits::GraphStorage;
    use post_cortex_core::core::context_update::EntityData;

    let storage = crate::SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let s = session(&[update(0, "a", &["Rust"])]).await;
    storage.save_session(&s).await.unwrap();
    let older = Utc::now() - Duration::days(30);
    storage
        .upsert_entity(
            s.id(),
            &EntityData {
                name: "rust".into(),
                entity_type: EntityType::Technology,
                first_mentioned: older,
                last_mentioned: older,
                mention_count: 4,
                importance_score: 1.0,
                description: None,
                aliases: Vec::new(),
            },
        )
        .await
        .unwrap();
    let loaded = storage.load_session(s.id()).await.unwrap();
    let graph = &loaded.entity_graph;
    assert_eq!(graph.entities.len(), 1);
    let rust = &graph.entities["rust"];
    assert_eq!(rust.aliases, ["Rust"], "the earlier spelling wins");
    assert_eq!(rust.mention_count, 5);
    assert_eq!(graph.resolve("Rust"), Some("rust"));
}
