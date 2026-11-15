// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Session persistence format (data version 3): a head row, records under
//! ordinals, graph rows, vectorized keys, listing records, and full session
//! deletion. A save writes what its changes name, so its batch does not
//! grow with the session (layouts of older versions are migrated at open:
//! `migrate/tests.rs`).

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::rows::SESSION_V2;
use crate::rocksdb_storage::session_rows::queue_save;
use crate::rocksdb_storage::types::StoredEmbedding;
use chrono::Utc;
use rocksdb::WriteBatch;
use std::collections::HashMap;
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity, UpdateContent,
    UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::VectorMetadata;
use post_cortex_ports::storage::{SessionChanges, SessionStore, VectorStore, WorkspaceStore};

/// A record naming the entities `entities`; the first two are related.
fn update_naming(title: &str, entities: &[&str]) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
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
        creates_relationships: match entities {
            [a, b, ..] => vec![EntityRelationship {
                from_entity: a.to_string(),
                to_entity: b.to_string(),
                relation_type: RelationType::RelatedTo,
                context: title.to_string(),
            }],
            _ => vec![],
        },
        references_entities: vec![],
        typed_entities: entities
            .iter()
            .map(|e| TypedEntity {
                name: e.to_string(),
                entity_type: EntityType::Concept,
            })
            .collect(),
        author: None,
        derived_from: Vec::new(),
    }
}

fn update(title: &str) -> ContextUpdate {
    update_naming(title, &[])
}

fn session_with(id: Uuid, updates: &[ContextUpdate]) -> ActiveSession {
    let mut session = ActiveSession::new(
        id,
        Some("name".into()),
        Some("desc".into()),
        chrono::Utc::now(),
    );
    for u in updates {
        session.apply_record(u.clone());
    }
    session
}

fn titles(session: &ActiveSession) -> Vec<String> {
    session
        .records()
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

fn keys_containing(storage: &RealRocksDBStorage, needle: &str) -> Vec<String> {
    storage
        .db
        .iterator(rocksdb::IteratorMode::Start)
        .map(|item| String::from_utf8_lossy(&item.unwrap().0).into_owned())
        .filter(|k| k.contains(needle))
        .collect()
}

async fn save_all(storage: &RealRocksDBStorage, session: &ActiveSession) {
    storage
        .save_session(session, SessionChanges::everything(session, &[]))
        .await
        .unwrap();
}

async fn save(storage: &RealRocksDBStorage, session: &ActiveSession, changes: SessionChanges) {
    storage.save_session(session, changes).await.unwrap();
}

fn writing(updates: Vec<ContextUpdate>) -> SessionChanges {
    SessionChanges {
        updates,
        ..Default::default()
    }
}

#[tokio::test]
async fn the_head_holds_no_records_and_a_load_keeps_session_order() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let updates: Vec<ContextUpdate> = (0..5).map(|i| update(&format!("u{i}"))).collect();
    let session = session_with(id, &updates);
    save_all(&storage, &session).await;

    let head = raw_get(&storage, &format!("session:{id}")).expect("head");
    assert!(head.starts_with(SESSION_V2));
    assert!(head.len() < 400, "{} bytes", head.len());
    assert_eq!(
        keys_containing(&storage, &format!("session:{id}:rec:")).len(),
        5
    );

    let loaded = storage.load_session(id).await.unwrap().session;
    assert_eq!(titles(&loaded), vec!["u0", "u1", "u2", "u3", "u4"]);
}

/// New records take the next ordinals in session order, whatever order the
/// save names them in; a record written again keeps its ordinal.
#[tokio::test]
async fn ordinals_follow_session_order_and_rewrites_keep_theirs() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let mut session = session_with(id, &[update("u0")]);
    save_all(&storage, &session).await;
    let (a, b) = (update("u1"), update("u2"));
    session.apply_record(a.clone());
    session.apply_record(b.clone());
    save(&storage, &session, writing(vec![b, a.clone()])).await;
    let mut changed = a;
    changed.content.description = "changed".into();
    session.replace_records(
        session
            .records_owned()
            .into_iter()
            .map(|u| {
                if u.id == changed.id {
                    changed.clone()
                } else {
                    u
                }
            })
            .collect(),
    );
    save(&storage, &session, writing(vec![changed])).await;

    let loaded = storage.load_session(id).await.unwrap().session;
    assert_eq!(titles(&loaded), vec!["u0", "u1", "u2"]);
    assert_eq!(loaded.records()[1].content.description, "changed");
    assert_eq!(
        keys_containing(&storage, &format!("session:{id}:rec:")).len(),
        3
    );
}

/// Removed records lose their rows and vectorized keys; a snapshot that
/// merely lacks a record (it predates it) never deletes it.
#[tokio::test]
async fn only_named_removals_delete_records() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let updates = vec![update("keep"), update("gone"), update("late")];
    let session = session_with(id, &updates);
    save(
        &storage,
        &session,
        SessionChanges {
            vectorized: updates.iter().map(|u| u.id).collect(),
            ..SessionChanges::everything(&session, &[])
        },
    )
    .await;

    let stale = session_with(id, &updates[..2]);
    save(
        &storage,
        &stale,
        SessionChanges {
            removed: vec![updates[1].id],
            ..Default::default()
        },
    )
    .await;
    let stored = storage.load_session(id).await.unwrap();
    assert_eq!(titles(&stored.session), vec!["keep", "late"]);
    let mut vectorized = stored.vectorized;
    vectorized.sort();
    let mut want = vec![updates[0].id, updates[2].id];
    want.sort();
    assert_eq!(vectorized, want);
    assert!(keys_containing(&storage, &updates[1].id.to_string()).is_empty());
}

/// The batch of a save carrying one new record is the same size in a
/// session of 10 records and in one of 500: a save costs what it changes.
#[tokio::test]
async fn a_save_writes_what_it_changes_whatever_the_session_size() {
    let (_dir, storage) = open().await;
    let mut sizes = Vec::new();
    for records in [10, 500] {
        let id = Uuid::new_v4();
        let updates: Vec<ContextUpdate> = (0..records)
            .map(|i| update_naming(&format!("r{i}"), &[&format!("e{}", i % 50), "hub"]))
            .collect();
        let mut session = session_with(id, &updates);
        save_all(&storage, &session).await;
        let new = update_naming("new", &["hub", "fresh"]);
        session.apply_record(new.clone());
        let mut batch = WriteBatch::default();
        queue_save(&storage.db, &mut batch, &session, &writing(vec![new])).unwrap();
        sizes.push((batch.len(), batch.size_in_bytes()));
        storage.db.write(batch).unwrap();
        let loaded = storage.load_session(id).await.unwrap().session;
        assert_eq!(
            loaded
                .entity_graph()
                .entities
                .get("fresh")
                .map(|e| e.mention_count),
            Some(1)
        );
        assert_eq!(
            loaded
                .entity_graph()
                .entities
                .get("hub")
                .map(|e| e.mention_count),
            Some(records as u32 + 1)
        );
    }
    assert_eq!(sizes[0].0, sizes[1].0, "{sizes:?}");
    assert!(sizes[1].1 < sizes[0].1 + 64, "{sizes:?}");
}

#[tokio::test]
async fn list_sessions_and_summaries_skip_update_rows() {
    let (_dir, storage) = open().await;
    let mut ids = Vec::new();
    for i in 0..4 {
        let id = Uuid::new_v4();
        let updates: Vec<ContextUpdate> = (0..i * 3).map(|j| update(&format!("{i}-{j}"))).collect();
        let mut session = session_with(id, &updates);
        session.set_name(Some(format!("session {i}")), chrono::Utc::now());
        save_all(&storage, &session).await;
        ids.push((id, updates.len()));
    }

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
}

#[tokio::test]
async fn delete_session_removes_every_record_of_the_session() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let other = Uuid::new_v4();
    for sid in [id, other] {
        let session = session_with(sid, &[update_naming("a", &["A", "B"]), update("b")]);
        save_all(&storage, &session).await;
        storage
            .upsert_vectors(vec![(
                vec![0.1; 8],
                VectorMetadata {
                    id: format!("vec-{sid}"),
                    text: "t".into(),
                    source: sid.to_string(),
                    content_type: "qa".into(),
                    timestamp: Utc::now(),
                    metadata: HashMap::new(),
                },
            )])
            .await
            .unwrap();
    }
    let workspace = Uuid::new_v4();
    storage
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "", &[])
        .await
        .unwrap();
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
    assert!(err.is_not_found());
    assert_eq!(err.to_string(), format!("not found: session {id}"));
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
    let row = |e: StoredEmbedding| {
        let metadata = e.to_metadata();
        (e.vector, metadata)
    };
    storage
        .upsert_vectors(vec![row(embedding("s1")), row(embedding("s2"))])
        .await
        .unwrap();
    assert_eq!(storage.total_count().await.unwrap(), 2);
    let shared = ["shared".to_string()];
    assert_eq!(storage.delete_vectors("s1", &shared).await.unwrap(), 1);
    assert_eq!(storage.delete_vectors("s1", &shared).await.unwrap(), 0);
    assert_eq!(storage.count_session_embeddings("s2").await.unwrap(), 1);
}

/// A record ordinal of another length than 8 bytes is corrupt.
#[tokio::test]
async fn a_damaged_ordinal_is_corrupt() {
    let (_dir, storage) = open().await;
    let id = Uuid::new_v4();
    let mut session = session_with(id, &[update("a")]);
    save_all(&storage, &session).await;
    storage
        .db
        .put(format!("session:{id}:next").as_bytes(), [1u8, 2])
        .unwrap();
    let late = update("late");
    session.apply_record(late.clone());
    let err = storage
        .save_session(&session, writing(vec![late]))
        .await
        .unwrap_err();
    assert!(err.is_corrupt(), "{err}");
}
