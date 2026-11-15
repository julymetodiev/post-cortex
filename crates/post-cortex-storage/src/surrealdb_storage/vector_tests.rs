// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embeddings in SurrealDB: session scoping, metadata,
//! and the HNSW index definition (dimension checks, redefinition, rebuild).

use super::SurrealDBStorage;
use super::persist_tests::test_storage;
use chrono::Utc;
use post_cortex_ports::VectorMetadata;
use post_cortex_ports::storage::VectorStore;
use std::collections::HashMap;
use uuid::Uuid;

fn vector_meta(content_id: &str, session: &str) -> VectorMetadata {
    VectorMetadata {
        id: content_id.to_string(),
        text: format!("{content_id} in {session}").into(),
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
        .db()
        .query("CREATE type::record('embedding', 'entity:rust') CONTENT $r")
        .bind((
            "r",
            crate::surrealdb_storage::vectors::embedding_record(
                v.clone(),
                vector_meta("entity:rust", &s1),
            ),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

    storage
        .upsert_vectors(vec![
            (v.clone(), vector_meta("entity:rust", &s1)),
            (v.clone(), vector_meta("entity:rust", &s2)),
        ])
        .await
        .unwrap();
    assert_eq!(storage.count_session_embeddings(&s1).await.unwrap(), 1);
    assert_eq!(storage.count_session_embeddings(&s2).await.unwrap(), 1);
    assert_eq!(storage.total_count().await.unwrap(), 2);

    let rust = ["entity:rust".to_string()];
    assert_eq!(storage.delete_vectors(&s1, &rust).await.unwrap(), 1);
    assert_eq!(storage.delete_vectors(&s1, &rust).await.unwrap(), 0);
    assert_eq!(storage.count_session_embeddings(&s2).await.unwrap(), 1);
}

fn text_meta(content_id: &str, session: &str, text: &str) -> VectorMetadata {
    VectorMetadata {
        text: text.into(),
        content_type: "UpdateContent".into(),
        ..vector_meta(content_id, session)
    }
}

/// Keys in an embedding's `metadata` survive a round trip (the field is an
/// `object` in a SCHEMAFULL table).
#[tokio::test]
async fn embedding_metadata_round_trips() {
    let storage = test_storage().await;
    let session = Uuid::new_v4().to_string();
    let mut meta = text_meta("m", &session, "metadata round trip");
    meta.metadata
        .insert("terms".into(), "decid keep rocksdb".into());
    storage
        .upsert_vectors(vec![(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], meta)])
        .await
        .unwrap();
    let loaded = storage.get_session_vectors(&session).await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded[0].1.metadata.get("terms").map(String::as_str),
        Some("decid keep rocksdb")
    );
}

/// The read-only open runs no schema statement: on a database without the
/// post-cortex schema it refuses instead of creating one.
#[tokio::test]
async fn open_existing_creates_no_schema() {
    let err = SurrealDBStorage::open_existing("mem://", None, None, None, None)
        .await
        .err()
        .expect("an empty database has no post-cortex schema")
        .to_string();
    assert!(err.contains("no post-cortex schema"), "{err}");

    let storage = test_storage().await;
    crate::surrealdb_storage::schema::check_initialised(&storage.db(), &storage.name)
        .await
        .unwrap();
}

/// Old databases had a non-FLEXIBLE `metadata` field: init upgrades it, and
/// leaves an already FLEXIBLE one untouched.
#[tokio::test]
async fn metadata_field_is_upgraded_only_when_needed() {
    let storage = test_storage().await;
    storage
        .db()
        .query("DEFINE FIELD OVERWRITE metadata ON embedding TYPE object;")
        .await
        .unwrap()
        .check()
        .unwrap();
    crate::surrealdb_storage::schema::initialize(&storage.db())
        .await
        .unwrap();
    let mut response = storage
        .db()
        .query("RETURN (INFO FOR TABLE embedding).fields.metadata;")
        .await
        .unwrap();
    let field: Option<String> = response.take(0).unwrap();
    assert!(field.unwrap().contains("FLEXIBLE"));
}
