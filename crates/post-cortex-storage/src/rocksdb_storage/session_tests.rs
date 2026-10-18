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

//! Session persistence format: row-backed updates, update index, meta
//! records, legacy-blob migration and full session deletion.

use super::RealRocksDBStorage;
use super::types::StoredEmbedding;
use chrono::Utc;
use std::collections::HashMap;
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

use crate::error::is_not_found;
use crate::traits::{GraphStorage, Storage, VectorStorage};
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityData, EntityRelationship, EntityType, RelationType, UpdateContent,
    UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;
use post_cortex_embeddings::VectorMetadata;

fn update(title: &str) -> ContextUpdate {
    ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: UpdateType::ConceptDefined,
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
        creates_entities: vec![],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
    }
}

fn session_with(id: Uuid, updates: &[ContextUpdate]) -> ActiveSession {
    let mut session = ActiveSession::new(id, Some("name".into()), Some("desc".into()));
    session.set_incremental_updates(updates.to_vec());
    session
}

fn titles(session: &ActiveSession) -> Vec<String> {
    session
        .incremental_updates
        .iter()
        .map(|u| u.content.title.clone())
        .collect()
}

async fn open() -> (TempDir, RealRocksDBStorage) {
    let dir = tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    (dir, storage)
}

fn raw_get(storage: &RealRocksDBStorage, key: &str) -> Option<Vec<u8>> {
    storage.db.get(key.as_bytes()).unwrap()
}

fn blob_update_count(storage: &RealRocksDBStorage, id: Uuid) -> usize {
    let blob = raw_get(storage, &format!("session:{id}")).expect("blob");
    let json: serde_json::Value = serde_json::from_slice(&blob).unwrap();
    json["incremental_updates"].as_array().unwrap().len()
}

fn keys_containing(storage: &RealRocksDBStorage, needle: &str) -> Vec<String> {
    storage
        .db
        .iterator(rocksdb::IteratorMode::Start)
        .map(|item| String::from_utf8_lossy(&item.unwrap().0).into_owned())
        .filter(|k| k.contains(needle))
        .collect()
}

#[tokio::test]
async fn blob_excludes_updates_and_load_rebuilds_them_in_order() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let updates: Vec<ContextUpdate> = (0..5).map(|i| update(&format!("u{i}"))).collect();
    let session = session_with(id, &updates);

    storage.save_session(&session).await.unwrap();

    assert_eq!(blob_update_count(&storage, id), 0);
    for u in &updates {
        assert!(raw_get(&storage, &format!("session:{id}:update:{}", u.id)).is_some());
    }

    let loaded = storage.load_session(id).await.unwrap();
    assert_eq!(titles(&loaded), vec!["u0", "u1", "u2", "u3", "u4"]);
    let rows = storage.load_session_updates(id).await.unwrap();
    assert_eq!(
        rows.iter().map(|u| u.id).collect::<Vec<_>>(),
        updates.iter().map(|u| u.id).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn every_persist_path_leaves_a_row_for_every_update() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let (u1, u2, u3) = (update("u1"), update("u2"), update("u3"));

    storage
        .save_session(&session_with(id, std::slice::from_ref(&u1)))
        .await
        .unwrap();

    // u2 is in the snapshot but not passed explicitly (e.g. its own persist
    // was coalesced or lost); it must still get a row.
    let snapshot = session_with(id, &[u1.clone(), u2.clone(), u3.clone()]);
    storage
        .save_session_with_updates(&snapshot, id, vec![u3.clone()])
        .await
        .unwrap();
    assert!(raw_get(&storage, &format!("session:{id}:update:{}", u2.id)).is_some());
    assert_eq!(
        titles(&storage.load_session(id).await.unwrap()),
        vec!["u1", "u2", "u3"]
    );

    // Plain re-save with an empty explicit list (vectorisation marks etc.).
    storage
        .save_session_with_updates(&snapshot, id, vec![])
        .await
        .unwrap();
    assert_eq!(
        storage
            .load_session(id)
            .await
            .unwrap()
            .incremental_updates
            .len(),
        3
    );
}

#[tokio::test]
async fn removed_updates_stay_removed_and_lose_their_rows() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let (u1, u2, u3) = (update("u1"), update("u2"), update("u3"));
    storage
        .save_session(&session_with(id, &[u1.clone(), u2.clone(), u3.clone()]))
        .await
        .unwrap();

    let mut session = storage.load_session(id).await.unwrap();
    assert!(session.remove_update_by_id(&u2.id));
    storage
        .save_session_with_updates(&session, id, vec![])
        .await
        .unwrap();

    assert!(raw_get(&storage, &format!("session:{id}:update:{}", u2.id)).is_none());
    let loaded = storage.load_session(id).await.unwrap();
    assert_eq!(titles(&loaded), vec!["u1", "u3"]);
    assert_eq!(storage.load_session_updates(id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn legacy_blob_with_embedded_updates_loads_and_is_migrated() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let updates = vec![update("old1"), update("old2"), update("old3")];
    let session = session_with(id, &updates);

    // What older versions wrote: the full blob, rows only for some updates.
    storage
        .db
        .put(
            format!("session:{id}").as_bytes(),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();
    storage
        .batch_save_updates(id, vec![updates[0].clone()])
        .await
        .unwrap();
    assert_eq!(blob_update_count(&storage, id), 3);

    let loaded = storage.load_session(id).await.unwrap();
    assert_eq!(titles(&loaded), vec!["old1", "old2", "old3"]);

    // Migrated: slim blob, every row present, index and meta written.
    assert_eq!(blob_update_count(&storage, id), 0);
    for u in &updates {
        assert!(raw_get(&storage, &format!("session:{id}:update:{}", u.id)).is_some());
    }
    assert!(raw_get(&storage, &format!("session_updates:{id}")).is_some());
    assert!(raw_get(&storage, &format!("session_meta:{id}")).is_some());

    let reloaded = storage.load_session(id).await.unwrap();
    assert_eq!(titles(&reloaded), vec!["old1", "old2", "old3"]);
}

#[tokio::test]
async fn blob_from_an_older_writer_wins_over_a_stale_index() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let (u1, u2, u3) = (update("u1"), update("u2"), update("u3"));
    storage
        .save_session(&session_with(id, &[u1.clone(), u2.clone()]))
        .await
        .unwrap();

    // A downgraded binary rewrites the full blob without touching the index.
    let newer = session_with(id, &[u1.clone(), u3.clone()]);
    storage
        .db
        .put(
            format!("session:{id}").as_bytes(),
            serde_json::to_vec(&newer).unwrap(),
        )
        .unwrap();

    assert_eq!(
        titles(&storage.load_session(id).await.unwrap()),
        vec!["u1", "u3"]
    );
    assert!(raw_get(&storage, &format!("session:{id}:update:{}", u2.id)).is_none());
    assert_eq!(
        titles(&storage.load_session(id).await.unwrap()),
        vec!["u1", "u3"]
    );
}

#[tokio::test]
async fn batch_save_updates_appends_to_a_row_backed_session() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let (u1, u2) = (update("u1"), update("u2"));
    storage
        .save_session(&session_with(id, std::slice::from_ref(&u1)))
        .await
        .unwrap();

    storage
        .batch_save_updates(id, vec![u1.clone(), u2.clone()])
        .await
        .unwrap();

    assert_eq!(
        titles(&storage.load_session(id).await.unwrap()),
        vec!["u1", "u2"]
    );
    let summaries = storage.list_session_summaries().await.unwrap();
    assert_eq!(summaries[0].update_count, 2);
}

#[tokio::test]
async fn list_sessions_and_summaries_skip_update_rows() {
    let (_dir, storage) = open().await;
    let mut ids = Vec::new();
    for i in 0..4 {
        let id = Uuid::new_v4();
        let updates: Vec<ContextUpdate> = (0..i * 3).map(|j| update(&format!("{i}-{j}"))).collect();
        let mut session = session_with(id, &updates);
        session.set_name(Some(format!("session {i}")));
        storage.save_session(&session).await.unwrap();
        ids.push((id, updates.len()));
    }

    // One legacy session without meta, index or rows.
    let legacy_id = Uuid::new_v4();
    let legacy = session_with(legacy_id, &[update("l1"), update("l2")]);
    storage
        .db
        .put(
            format!("session:{legacy_id}").as_bytes(),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
    ids.push((legacy_id, 2));

    let mut listed = storage.list_sessions().await.unwrap();
    listed.sort();
    let mut expected: Vec<Uuid> = ids.iter().map(|(id, _)| *id).collect();
    expected.sort();
    assert_eq!(listed, expected);

    let summaries = storage.list_session_summaries().await.unwrap();
    assert_eq!(summaries.len(), ids.len());
    for (id, count) in &ids {
        let s = summaries.iter().find(|s| s.id == *id).unwrap();
        assert_eq!(s.update_count, *count, "update count of {id}");
    }
    let legacy_summary = summaries.iter().find(|s| s.id == legacy_id).unwrap();
    assert_eq!(legacy_summary.name.as_deref(), Some("name"));
    // Lazily backfilled, and the legacy blob itself is left alone.
    assert!(raw_get(&storage, &format!("session_meta:{legacy_id}")).is_some());
    assert_eq!(blob_update_count(&storage, legacy_id), 2);
}

#[tokio::test]
async fn delete_session_removes_every_record_of_the_session() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let other = Uuid::new_v4();
    for sid in [id, other] {
        storage
            .save_session(&session_with(sid, &[update("a"), update("b")]))
            .await
            .unwrap();
        storage
            .add_vector(
                vec![0.1; 8],
                VectorMetadata {
                    id: format!("vec-{sid}"),
                    text: "t".into(),
                    source: sid.to_string(),
                    content_type: "qa".into(),
                    timestamp: Utc::now(),
                    metadata: HashMap::new(),
                },
            )
            .await
            .unwrap();
        for name in ["A", "B"] {
            storage
                .upsert_entity(
                    sid,
                    &EntityData {
                        name: name.into(),
                        entity_type: EntityType::Concept,
                        first_mentioned: Utc::now(),
                        last_mentioned: Utc::now(),
                        mention_count: 1,
                        importance_score: 0.5,
                        description: None,
                    },
                )
                .await
                .unwrap();
        }
        storage
            .create_relationship(
                sid,
                &EntityRelationship {
                    from_entity: "A".into(),
                    to_entity: "B".into(),
                    relation_type: RelationType::RelatedTo,
                    context: String::new(),
                },
            )
            .await
            .unwrap();
    }
    let workspace = Uuid::new_v4();
    storage
        .add_session_to_workspace(workspace, id, SessionRole::Primary)
        .await
        .unwrap();
    storage
        .add_session_to_workspace(workspace, other, SessionRole::Related)
        .await
        .unwrap();
    let before_other = keys_containing(&storage, &other.to_string());

    storage.delete_session(id).await.unwrap();

    assert_eq!(
        keys_containing(&storage, &id.to_string()),
        Vec::<String>::new()
    );
    assert_eq!(keys_containing(&storage, &other.to_string()), before_other);
    assert!(!storage.session_exists(id).await.unwrap());
    let err = storage.load_session(id).await.unwrap_err();
    assert!(is_not_found(&err));
    assert!(err.to_string().contains("Session not found"));
}

#[tokio::test]
async fn clear_session_entities_drops_entities_and_relationships() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    for name in ["A", "B"] {
        storage
            .upsert_entity(
                id,
                &EntityData {
                    name: name.into(),
                    entity_type: EntityType::Concept,
                    first_mentioned: Utc::now(),
                    last_mentioned: Utc::now(),
                    mention_count: 1,
                    importance_score: 0.5,
                    description: None,
                },
            )
            .await
            .unwrap();
    }
    storage
        .create_relationship(
            id,
            &EntityRelationship {
                from_entity: "A".into(),
                to_entity: "B".into(),
                relation_type: RelationType::DependsOn,
                context: String::new(),
            },
        )
        .await
        .unwrap();

    Storage::clear_session_entities(&storage, id).await.unwrap();

    assert!(storage.list_entities(id).await.unwrap().is_empty());
    assert!(
        storage
            .find_related_entities(id, "A")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn same_content_id_in_two_sessions_is_removed_per_session() {
    let (_dir, storage) = open().await;
    let embedding = |sid: &str| StoredEmbedding {
        content_id: "shared".into(),
        session_id: sid.into(),
        vector: vec![1.0; 4],
        text: String::new(),
        content_type: "qa".into(),
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    };
    storage.save_embedding(&embedding("s1")).await.unwrap();
    storage.save_embedding(&embedding("s2")).await.unwrap();
    assert_eq!(storage.total_count().await, 2);
    assert!(storage.remove_session_vector("s1", "shared").await.unwrap());
    assert!(!storage.remove_session_vector("s1", "shared").await.unwrap());
    assert_eq!(storage.count_session_embeddings("s2").await, 1);
    // Unscoped removal takes every session's copy.
    storage.save_embedding(&embedding("s1")).await.unwrap();
    assert!(storage.remove_vector("shared").await.unwrap());
    assert_eq!(storage.total_count().await, 0);
}
