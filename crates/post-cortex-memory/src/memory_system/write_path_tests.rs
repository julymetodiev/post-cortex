//! Write-path tests for [`ConversationMemorySystem`]: load failures,
//! single application of derived work, persist ordering, cache-miss races
//! and deletes.

use std::sync::Arc;
use std::time::Duration;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity, UpdateContent,
    UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_storage::rocksdb_storage::RealRocksDBStorage;
use uuid::Uuid;

use super::{ConversationMemorySystem, SystemConfig};

fn config(dir: &tempfile::TempDir, embeddings: bool) -> SystemConfig {
    SystemConfig {
        data_directory: dir.path().to_string_lossy().into_owned(),
        enable_embeddings: embeddings,
        ..Default::default()
    }
}

async fn system(embeddings: bool) -> (Arc<ConversationMemorySystem>, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("pcx_memw_")
        .tempdir()
        .unwrap();
    let sys = ConversationMemorySystem::new(config(&dir, embeddings))
        .await
        .unwrap();
    (Arc::new(sys), dir)
}

/// Metadata for an update that mentions `Foo` and `Bar`.
fn update_metadata(title: &str) -> serde_json::Value {
    let update = ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: title.to_string(),
            description: format!("{title}: a description long enough to be vectorized"),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec!["Foo".into(), "Bar".into()],
        creates_relationships: vec![EntityRelationship {
            from_entity: "Foo".into(),
            to_entity: "Bar".into(),
            relation_type: RelationType::RelatedTo,
            context: "test".into(),
        }],
        references_entities: vec![],
        typed_entities: vec![
            TypedEntity {
                name: "Foo".into(),
                entity_type: EntityType::Concept,
            },
            TypedEntity {
                name: "Bar".into(),
                entity_type: EntityType::Concept,
            },
        ],
    };
    serde_json::to_value(update).unwrap()
}

fn mentions(session: &ActiveSession, entity: &str) -> u32 {
    session
        .entity_graph
        .entities
        .get(entity)
        .map_or(0, |e| e.mention_count)
}

/// Poll until `check` holds or ~2 s passed.
async fn eventually(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

// ----- W1: a load failure must never be answered with an empty session ---

#[tokio::test]
async fn load_failure_is_an_error_and_never_overwrites_the_stored_session() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = Uuid::new_v4();

    // Store a session, then corrupt its blob behind the backend's back.
    {
        let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
        let session = ActiveSession::new(session_id, Some("precious".into()), None);
        storage.save_session(&session).await.unwrap();
    }
    {
        let db = rocksdb::DB::open_default(dir.path().join("rocksdb")).unwrap();
        db.put(format!("session:{session_id}"), b"{ not a session")
            .unwrap();
    }

    let sys = ConversationMemorySystem::new(config(&dir, false))
        .await
        .unwrap();

    let err = sys
        .session_manager
        .get_or_create_session(session_id)
        .await
        .expect_err("a corrupt session must not load or be recreated");
    assert!(err.contains("Storage error"), "{err}");
    assert!(
        sys.add_incremental_update(session_id, "would clobber".into(), None)
            .await
            .is_err()
    );
    assert!(sys.get_session(session_id).await.is_err());

    // Still the corrupt blob: nothing saved an empty session over it.
    let still = sys.storage_actor.load_session(session_id).await;
    assert!(still.is_err(), "stored session was replaced: {still:?}");
}

#[tokio::test]
async fn missing_session_is_created_on_first_write() {
    let (sys, _dir) = system(false).await;
    let session_id = Uuid::new_v4();
    assert!(sys.get_session(session_id).await.is_err());

    sys.add_incremental_update(session_id, "first".into(), None)
        .await
        .unwrap();
    assert!(
        sys.storage_actor
            .load_session(session_id)
            .await
            .unwrap()
            .is_some()
    );
}

// ----- W2: the graph merge runs exactly once per update ------------------

#[tokio::test]
async fn direct_update_merges_entities_once() {
    let (sys, _dir) = system(false).await;
    let sid = sys.create_session(None, None).await.unwrap();

    for (i, title) in ["one", "two", "three"].into_iter().enumerate() {
        sys.add_incremental_update(sid, title.into(), Some(update_metadata(title)))
            .await
            .unwrap();
        let session = sys.get_session(sid).await.unwrap().load_full();
        assert_eq!(mentions(&session, "Foo"), i as u32 + 1);
    }

    // Nothing applies it again in the background.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let session = sys.get_session(sid).await.unwrap().load_full();
    assert_eq!(mentions(&session, "Foo"), 3);
}

#[tokio::test]
async fn graph_merge_retries_a_lost_swap_instead_of_dropping_it() {
    let (sys, _dir) = system(false).await;
    let sid = sys.create_session(None, None).await.unwrap();

    // Deferred writes leave the merge to the caller; run all merges and
    // more writes concurrently so swaps collide.
    let mut committed = Vec::new();
    for i in 0..8 {
        let title = format!("u{i}");
        let c = sys
            .add_incremental_update_deferred(
                sid,
                title.clone(),
                Some(update_metadata(&title)),
                false,
            )
            .await
            .unwrap();
        committed.push(c.update);
    }
    let merges: Vec<_> = committed
        .into_iter()
        .map(|update| {
            let sys = Arc::clone(&sys);
            tokio::spawn(async move { sys.apply_entity_graph_update_now(sid, update).await })
        })
        .collect();
    let writes: Vec<_> = (0..8)
        .map(|i| {
            let sys = Arc::clone(&sys);
            tokio::spawn(async move {
                sys.add_incremental_update_deferred(sid, format!("w{i}"), None, false)
                    .await
                    .map(|_| ())
            })
        })
        .collect();
    for r in futures::future::join_all(merges.into_iter().chain(writes)).await {
        r.unwrap().unwrap();
    }

    let session = sys.get_session(sid).await.unwrap().load_full();
    assert_eq!(mentions(&session, "Foo"), 8, "every merge applied once");
    assert_eq!(session.incremental_updates.len(), 16);
}

// ----- W3: persists are ordered, the stored session ends at the newest ---

#[tokio::test]
async fn rapid_writes_leave_the_newest_session_in_storage() {
    let (sys, _dir) = system(false).await;
    let sid = sys.create_session(None, None).await.unwrap();

    const K: usize = 30;
    for i in 0..K {
        sys.add_incremental_update(
            sid,
            format!("update {i}"),
            Some(update_metadata(&format!("t{i}"))),
        )
        .await
        .unwrap();
    }
    let live = sys.get_session(sid).await.unwrap();
    sys.storage_actor
        .persist_session_and_wait(&live, vec![])
        .await
        .unwrap();

    let stored = sys.storage_actor.load_session(sid).await.unwrap().unwrap();
    assert_eq!(stored.incremental_updates.len(), K);
    assert_eq!(
        mentions(&stored, "Foo"),
        K as u32,
        "graph of the newest version"
    );
}

// ----- W5: concurrent cache misses share one handle ----------------------

#[tokio::test]
async fn concurrent_cache_misses_return_the_same_handle() {
    let (sys, _dir) = system(false).await;

    // A session only in storage, and one that does not exist yet.
    let stored = sys.create_session(None, None).await.unwrap();
    sys.session_manager.sessions.remove(&stored);
    let fresh = Uuid::new_v4();

    for session_id in [stored, fresh] {
        let handles = futures::future::join_all((0..16).map(|_| {
            let sys = Arc::clone(&sys);
            tokio::spawn(async move {
                sys.session_manager
                    .get_or_create_session(session_id)
                    .await
                    .unwrap()
            })
        }))
        .await;
        let first = handles[0].as_ref().unwrap();
        for h in &handles {
            assert!(Arc::ptr_eq(first, h.as_ref().unwrap()));
        }
        let cached = sys.session_manager.sessions.get(&session_id).unwrap();
        assert!(Arc::ptr_eq(first, &cached));
    }
}

// ----- W7: durability is reported truthfully ------------------------------

#[tokio::test]
async fn deferred_write_waits_for_storage_when_asked() {
    let (sys, _dir) = system(false).await;
    let sid = sys.create_session(None, None).await.unwrap();

    let c = sys
        .add_incremental_update_deferred(sid, "durable".into(), Some(update_metadata("d")), true)
        .await
        .unwrap();
    assert!(c.durable);
    let stored = sys.storage_actor.load_session(sid).await.unwrap().unwrap();
    assert!(
        stored
            .incremental_updates
            .iter()
            .any(|u| u.id == c.entry_id)
    );

    let c = sys
        .add_incremental_update_deferred(sid, "later".into(), None, false)
        .await
        .unwrap();
    assert!(!c.durable, "not waited for, so not claimed");
}

// ----- W8: listing reads metadata only ------------------------------------

#[tokio::test]
async fn listing_and_search_do_not_load_sessions() {
    let (sys, _dir) = system(false).await;
    let a = sys
        .create_session(Some("Alpha project".into()), Some("first".into()))
        .await
        .unwrap();
    let b = sys
        .create_session(Some("Beta".into()), Some("about alpha too".into()))
        .await
        .unwrap();
    let c = sys
        .create_session(Some("Gamma".into()), None)
        .await
        .unwrap();
    for id in [a, b, c] {
        sys.session_manager.sessions.remove(&id);
    }

    let all = sys.list_session_summaries().await.unwrap();
    assert_eq!(all.iter().map(|s| s.id).collect::<Vec<_>>(), vec![a, b, c]);

    let found = sys
        .find_sessions_by_name_or_description("ALPHA")
        .await
        .unwrap();
    assert_eq!(found, vec![a, b]);
    assert!(
        sys.session_manager.sessions.is_empty(),
        "nothing was cached"
    );

    // A cached session reports its in-memory state.
    sys.update_session_metadata(c, Some("Gamma alpha".into()), None)
        .await
        .unwrap();
    let found = sys
        .find_sessions_by_name_or_description("alpha")
        .await
        .unwrap();
    assert_eq!(found, vec![a, b, c]);
}

// ----- W9: a delete removes vectors and cannot be undone by late writes ---

#[tokio::test]
async fn delete_removes_vectors_and_late_work_cannot_resurrect_the_session() {
    let (sys, _dir) = system(true).await;
    let sid = sys.create_session(None, None).await.unwrap();

    let first = sys
        .add_incremental_update_deferred(sid, "first".into(), Some(update_metadata("first")), true)
        .await
        .unwrap();
    let second = sys
        .add_incremental_update_deferred(
            sid,
            "second".into(),
            Some(update_metadata("second")),
            true,
        )
        .await
        .unwrap();
    assert!(sys.vectorize_update_now(sid, first.entry_id).await.unwrap() > 0);

    let vectorizer = sys.ensure_vectorizer_initialized().await.unwrap();
    assert!(vectorizer.count_session_embeddings(sid) > 0);
    assert!(
        sys.vector_storage
            .count_session_embeddings(&sid.to_string())
            .await
            > 0
    );

    let old_handle = sys.get_session(sid).await.unwrap();
    assert!(sys.delete_session(sid).await.unwrap());

    assert_eq!(vectorizer.count_session_embeddings(sid), 0);
    assert_eq!(
        sys.vector_storage
            .count_session_embeddings(&sid.to_string())
            .await,
        0
    );

    // Work still holding the old handle: a vectorization that finishes
    // after the delete, and a persist.
    sys.spawn_background_vectorization(sid, Arc::clone(&old_handle), second.entry_id)
        .await;
    let persisted = sys
        .storage_actor
        .persist_session_and_wait(&old_handle, vec![])
        .await;
    assert!(
        persisted.is_err(),
        "persist of a deleted session must be refused"
    );

    assert!(
        eventually(|| old_handle
            .load()
            .vectorized_update_ids
            .contains(&second.entry_id))
        .await,
        "background vectorization ran"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(vectorizer.count_session_embeddings(sid), 0);
    assert_eq!(
        sys.vector_storage
            .count_session_embeddings(&sid.to_string())
            .await,
        0
    );
    assert!(sys.storage_actor.load_session(sid).await.unwrap().is_none());
    assert!(sys.get_session(sid).await.is_err());
}

#[tokio::test]
async fn a_handle_held_outside_the_cache_cannot_resurrect_a_deleted_session() {
    let (sys, _dir) = system(false).await;
    let sid = sys.create_session(None, None).await.unwrap();

    // A worker loaded the session, then it left the cache (live = None at
    // delete time).
    let held = sys.get_session(sid).await.unwrap();
    sys.session_manager.sessions.remove(&sid);
    assert!(sys.delete_session(sid).await.unwrap());

    let mut next = (**held.load()).clone();
    next.set_name(Some("ghost".into()));
    held.store(Arc::new(next));
    assert!(
        sys.storage_actor
            .persist_session_and_wait(&held, vec![])
            .await
            .is_err()
    );
    assert!(sys.storage_actor.load_session(sid).await.unwrap().is_none());
}

#[tokio::test]
async fn loads_racing_a_delete_never_bring_the_session_back() {
    let (sys, _dir) = system(false).await;
    for round in 0..20 {
        let sid = sys.create_session(None, None).await.unwrap();
        sys.add_incremental_update(sid, format!("round {round}"), None)
            .await
            .unwrap();
        let live = sys.get_session(sid).await.unwrap();
        sys.storage_actor
            .persist_session_and_wait(&live, vec![])
            .await
            .unwrap();
        sys.session_manager.sessions.remove(&sid);

        // Pipeline-worker-like tasks: load (cache miss), modify, persist.
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let sys = Arc::clone(&sys);
                tokio::spawn(async move {
                    if let Ok(Some(h)) = sys.session_manager.get_session(sid).await {
                        let mut next = (**h.load()).clone();
                        next.set_name(Some("worker".into()));
                        h.store(Arc::new(next));
                        let _ = sys.storage_actor.persist_session_and_wait(&h, vec![]).await;
                    }
                })
            })
            .collect();
        tokio::task::yield_now().await;
        assert!(sys.delete_session(sid).await.unwrap());
        for w in workers {
            w.await.unwrap();
        }

        assert!(
            sys.storage_actor.load_session(sid).await.unwrap().is_none(),
            "round {round}: session written back after its delete"
        );
    }
}

// ----- Eviction and reload never lose or delete acknowledged updates -----

#[tokio::test]
async fn reload_after_eviction_sees_queued_writes_and_stale_copies_delete_nothing() {
    let (sys, _dir) = system(false).await;
    let sid = sys.create_session(None, None).await.unwrap();

    // Queued, not yet written; then the cache drops the handle (capacity
    // eviction does not flush).
    let first = sys
        .add_incremental_update_deferred(sid, "first".into(), Some(update_metadata("a")), false)
        .await
        .unwrap();
    let stale = sys.get_session(sid).await.unwrap();
    sys.session_manager.sessions.remove(&sid);

    let reloaded = sys.get_session(sid).await.unwrap();
    assert!(!Arc::ptr_eq(&stale, &reloaded));
    assert!(
        reloaded
            .load()
            .incremental_updates
            .iter()
            .any(|u| u.id == first.entry_id),
        "the reload waited for the evicted copy's queued write"
    );

    // A new update on the reloaded copy, then a late persist of the stale
    // copy (which lacks it): the update must survive.
    let second = sys
        .add_incremental_update_deferred(sid, "second".into(), None, true)
        .await
        .unwrap();
    assert!(second.durable);
    let _ = sys
        .storage_actor
        .persist_session_and_wait(&stale, vec![])
        .await;
    let stored = sys.storage_actor.load_session(sid).await.unwrap().unwrap();
    let ids: Vec<Uuid> = stored.incremental_updates.iter().map(|u| u.id).collect();
    assert!(ids.contains(&first.entry_id) && ids.contains(&second.entry_id));

    // Explicit removal still deletes.
    assert!(sys.delete_update(sid, first.entry_id).await.unwrap());
    let live = sys.get_session(sid).await.unwrap();
    sys.storage_actor
        .persist_session_and_wait(&live, vec![])
        .await
        .unwrap();
    let stored = sys.storage_actor.load_session(sid).await.unwrap().unwrap();
    assert!(
        !stored
            .incremental_updates
            .iter()
            .any(|u| u.id == first.entry_id)
    );
}

#[tokio::test]
async fn eviction_and_shutdown_flush_pending_writes() {
    let (sys, _dir) = system(false).await;
    let a = sys.create_session(None, None).await.unwrap();
    let b = sys.create_session(None, None).await.unwrap();

    let ua = sys
        .add_incremental_update_deferred(a, "a".into(), None, false)
        .await
        .unwrap();
    sys.force_memory_cleanup(0).await;
    let stored = sys.storage_actor.load_session(a).await.unwrap().unwrap();
    assert!(
        stored
            .incremental_updates
            .iter()
            .any(|u| u.id == ua.entry_id),
        "evicting flushed the session first"
    );

    let ub = sys
        .add_incremental_update_deferred(b, "b".into(), None, false)
        .await
        .unwrap();
    assert_eq!(sys.flush_all(Duration::from_secs(10)).await, 0);
    let stored = sys.storage_actor.load_session(b).await.unwrap().unwrap();
    assert!(
        stored
            .incremental_updates
            .iter()
            .any(|u| u.id == ub.entry_id)
    );
}
