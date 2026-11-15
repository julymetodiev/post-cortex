// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::memory_system::SystemConfig;
use crate::memory_system::write_path_tests::text_record;
use post_cortex_core::core::context_update::EntityType;
use post_cortex_ports::VectorMetadata;
use std::time::Duration;

async fn system(
    suffix: &str,
) -> (
    crate::memory_system::backends_tests::TestSystem,
    tempfile::TempDir,
) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("pcx_mems_{suffix}_"))
        .tempdir()
        .unwrap();
    let config = SystemConfig {
        // No model load: these tests cover the paths that must work
        // before (or without) the vectorizer.
        enable_embeddings: false,
        ..Default::default()
    };
    (
        crate::memory_system::backends_tests::rocksdb_system(
            &dir,
            config,
            crate::memory_system::backends_tests::default_engine(),
        )
        .await,
        dir,
    )
}

/// Persist an embedding for `content_id`, in whatever dimension the
/// storage backend accepts.
async fn persist_embedding(
    sys: &crate::memory_system::backends_tests::TestSystem,
    session_id: Uuid,
    content_id: &str,
) {
    for dim in [256, 384] {
        let metadata = VectorMetadata::new(
            content_id.to_string(),
            "text".to_string(),
            session_id.to_string(),
            "UpdateContent".to_string(),
        );
        if sys
            .stored()
            .upsert_vectors(vec![(vec![0.5; dim], metadata)])
            .await
            .is_ok()
        {
            return;
        }
    }
    panic!("storage accepted no embedding dimension");
}

async fn persisted_count(
    sys: &crate::memory_system::backends_tests::TestSystem,
    session_id: Uuid,
) -> usize {
    sys.stored()
        .count_session_embeddings(&session_id.to_string())
        .await
        .unwrap()
}

/// Poll the stored session until `check` holds (persists are queued).
async fn wait_for_stored(
    sys: &crate::memory_system::backends_tests::TestSystem,
    session_id: Uuid,
    check: impl Fn(&post_cortex_core::session::active_session::ActiveSession) -> bool,
) -> bool {
    for _ in 0..100 {
        if let Ok(Some(stored)) = sys.session_manager.load_stored(session_id).await
            && check(&stored.session)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn delete_update_removes_persisted_embedding() {
    let (sys, _dir) = system("delupd").await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();
    sys.mem
        .records
        .commit(sid, text_record("an update worth keeping"), false)
        .await
        .unwrap();
    let entry_id = sys.get_session(sid).await.unwrap().load().records()[0].id;
    persist_embedding(&sys, sid, &entry_id.to_string()).await;
    assert_eq!(persisted_count(&sys, sid).await, 1);

    assert!(sys.mem.records.delete(sid, entry_id).await.unwrap());
    assert!(!sys.mem.records.delete(sid, entry_id).await.unwrap());

    assert_eq!(
        persisted_count(&sys, sid).await,
        0,
        "deleted update's embedding would be reloaded on restart"
    );
    let session = sys.get_session(sid).await.unwrap().load_full();
    assert!(session.records().is_empty());
}

#[tokio::test]
async fn delete_entity_removes_persisted_embedding_and_persists_session() {
    let (sys, _dir) = system("delent").await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();
    {
        let arc = sys.get_session(sid).await.unwrap();
        let mut session = (**arc.load()).clone();
        session.entity_graph_mut().add_or_update_entity(
            "Doomed".into(),
            EntityType::Concept,
            chrono::Utc::now(),
            "",
        );
        arc.store(Arc::new(session));
    }
    persist_embedding(&sys, sid, "entity:Doomed").await;

    assert!(sys.mem.entities.delete(sid, "Doomed").await.unwrap());
    assert!(!sys.mem.entities.delete(sid, "Doomed").await.unwrap());
    assert_eq!(persisted_count(&sys, sid).await, 0);

    let persisted = wait_for_stored(&sys, sid, |s| !s.entity_graph().has_entity("Doomed")).await;
    assert!(persisted, "entity removal was not persisted");
}

#[tokio::test]
async fn delete_entity_keeps_other_sessions_embedding_of_the_same_name() {
    let (sys, _dir) = system("delent_scope").await;
    let a = sys.mem.sessions.create(None, None).await.unwrap();
    let b = sys.mem.sessions.create(None, None).await.unwrap();
    for sid in [a, b] {
        let arc = sys.get_session(sid).await.unwrap();
        let mut session = (**arc.load()).clone();
        session.entity_graph_mut().add_or_update_entity(
            "Rust".into(),
            EntityType::Technology,
            chrono::Utc::now(),
            "",
        );
        arc.store(Arc::new(session));
        persist_embedding(&sys, sid, "entity:Rust").await;
    }

    assert!(sys.mem.entities.delete(a, "Rust").await.unwrap());
    assert_eq!(persisted_count(&sys, a).await, 0);
    assert_eq!(
        persisted_count(&sys, b).await,
        1,
        "another session's entity:Rust must survive"
    );
}

#[tokio::test]
async fn update_session_metadata_persists() {
    let (sys, _dir) = system("meta").await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    sys.mem
        .sessions
        .update_metadata(sid, Some("renamed".into()), Some("desc".into()))
        .await
        .unwrap();
    let session = sys.get_session(sid).await.unwrap().load_full();
    assert_eq!(session.name().as_deref(), Some("renamed"));

    let persisted = wait_for_stored(&sys, sid, |s| {
        s.name().as_deref() == Some("renamed") && s.description().as_deref() == Some("desc")
    })
    .await;
    assert!(persisted, "metadata change was not persisted");
}
