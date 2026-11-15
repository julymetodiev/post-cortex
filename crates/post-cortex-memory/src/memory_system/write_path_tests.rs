//! Write-path tests for [`MemoryRuntime`]: load failures,
//! single application of derived work, persist ordering, cache-miss races
//! and deletes.

use post_cortex_ports::SessionStore;
use std::sync::Arc;
use std::time::Duration;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity, UpdateContent,
    UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::SessionSummary;
use post_cortex_storage::rocksdb_storage::RealRocksDBStorage;
use uuid::Uuid;

use super::SystemConfig;
use super::backends_tests::TestSystem;

pub(super) fn config(_dir: &tempfile::TempDir, embeddings: bool) -> SystemConfig {
    SystemConfig {
        enable_embeddings: embeddings,
        ..Default::default()
    }
}

pub(super) async fn system(embeddings: bool) -> (TestSystem, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("pcx_memw_")
        .tempdir()
        .unwrap();
    let sys = crate::memory_system::backends_tests::rocksdb_system(
        &dir,
        config(&dir, embeddings),
        crate::memory_system::backends_tests::default_engine(),
    )
    .await;
    (sys, dir)
}

/// A record that mentions `Foo` and `Bar`.
pub(super) fn update_metadata(title: &str) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
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
        author: None,
        derived_from: Vec::new(),
    }
}

/// A concept record whose description is `text`, naming no entity.
pub(crate) fn text_record(text: &str) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: "Incremental Update".to_string(),
            description: text.to_string(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: Vec::new(),
        creates_relationships: Vec::new(),
        references_entities: Vec::new(),
        typed_entities: Vec::new(),
        author: None,
        derived_from: Vec::new(),
    }
}

pub(super) fn mentions(session: &ActiveSession, entity: &str) -> u32 {
    session
        .entity_graph()
        .entities
        .get(entity)
        .map_or(0, |e| e.mention_count)
}

// ----- W1: a load failure must never be answered with an empty session ---

#[tokio::test]
async fn load_failure_is_an_error_and_never_overwrites_the_stored_session() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = Uuid::new_v4();

    // Store a session, then corrupt its blob behind the backend's back.
    {
        let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
        let session = ActiveSession::new(
            session_id,
            Some("precious".into()),
            None,
            chrono::Utc::now(),
        );
        storage
            .save_session(
                &session,
                post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
            )
            .await
            .unwrap();
    }
    {
        let db = rocksdb::DB::open_default(dir.path().join("rocksdb")).unwrap();
        db.put(format!("session:{session_id}"), b"{ not a session")
            .unwrap();
    }

    let sys = TestSystem::new(
        crate::memory_system::backends_tests::rocksdb_runtime(&dir, config(&dir, false))
            .await
            .unwrap(),
    );

    let err = sys
        .session_manager
        .get_session(session_id)
        .await
        .expect_err("a corrupt session must not load or be recreated");
    assert!(
        matches!(
            err,
            crate::memory_system::RuntimeError::Port(post_cortex_ports::PortError::Corrupt { .. })
        ),
        "{err}"
    );
    assert!(
        sys.mem
            .records
            .commit(session_id, text_record("would clobber"), false)
            .await
            .is_err()
    );
    assert!(sys.get_session(session_id).await.is_err());

    // Still the corrupt blob: nothing saved an empty session over it.
    let still = sys.session_manager.load_stored(session_id).await;
    assert!(still.is_err(), "stored session was replaced: {still:?}");
}

// ----- W2: the graph merge runs exactly once per update, in the write ----

#[tokio::test]
async fn direct_update_merges_entities_once() {
    let (sys, _dir) = system(false).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    for (i, title) in ["one", "two", "three"].into_iter().enumerate() {
        sys.mem
            .records
            .commit(sid, update_metadata(title), false)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_merge_every_graph_update_once() {
    let (sys, _dir) = system(false).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    // Concurrent writes so swaps collide: each write's graph merge is in
    // its own swap, retried on the newer version, never lost or doubled.
    let writes: Vec<_> = (0..16)
        .map(|i| {
            let sys = sys.clone();
            tokio::spawn(async move {
                let title = format!("u{i}");
                let record = if i % 2 == 0 {
                    update_metadata(&title)
                } else {
                    text_record(&title)
                };
                sys.mem.records.commit(sid, record, false).await.map(|_| ())
            })
        })
        .collect();
    for r in futures::future::join_all(writes).await {
        r.unwrap().unwrap();
    }

    let session = sys.get_session(sid).await.unwrap().load_full();
    assert_eq!(mentions(&session, "Foo"), 8, "every merge applied once");
    assert_eq!(session.records().len(), 16);
}

// ----- W3: persists are ordered, the stored session ends at the newest ---

#[tokio::test]
async fn rapid_writes_leave_the_newest_session_in_storage() {
    let (sys, _dir) = system(false).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    const K: usize = 30;
    for i in 0..K {
        sys.mem
            .records
            .commit(sid, update_metadata(&format!("t{i}")), false)
            .await
            .unwrap();
    }
    let live = sys.get_session(sid).await.unwrap();
    sys.persister.persist_and_wait(&live, vec![]).await.unwrap();

    let stored = sys
        .session_manager
        .load_stored(sid)
        .await
        .unwrap()
        .unwrap()
        .session;
    assert_eq!(stored.records().len(), K);
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

    // A session only in storage.
    let session_id = sys.mem.sessions.create(None, None).await.unwrap();
    sys.session_manager.sessions.remove(&session_id);

    let handles = futures::future::join_all((0..16).map(|_| {
        let sys = sys.clone();
        tokio::spawn(async move { sys.get_session(session_id).await.unwrap() })
    }))
    .await;
    let first = handles[0].as_ref().unwrap();
    for h in &handles {
        assert!(Arc::ptr_eq(first, h.as_ref().unwrap()));
    }
    let cached = sys.session_manager.sessions.get(&session_id).unwrap();
    assert!(Arc::ptr_eq(first, &cached));
}

// ----- W7: durability is reported truthfully ------------------------------

#[tokio::test]
async fn deferred_write_waits_for_storage_when_asked() {
    let (sys, _dir) = system(false).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    let c = sys
        .mem
        .records
        .commit(sid, update_metadata("d"), true)
        .await
        .unwrap();
    assert!(c.durable);
    let stored = sys
        .session_manager
        .load_stored(sid)
        .await
        .unwrap()
        .unwrap()
        .session;
    assert!(stored.records().iter().any(|u| u.id == c.entry_id));

    let c = sys
        .mem
        .records
        .commit(sid, text_record("later"), false)
        .await
        .unwrap();
    assert!(!c.durable, "not waited for, so not claimed");
}

// ----- W8: listing reads metadata only ------------------------------------

#[tokio::test]
async fn listing_and_search_do_not_load_sessions() {
    let (sys, _dir) = system(false).await;
    let a = sys
        .mem
        .sessions
        .create(Some("Alpha project".into()), Some("first".into()))
        .await
        .unwrap();
    let b = sys
        .mem
        .sessions
        .create(Some("Beta".into()), Some("about alpha too".into()))
        .await
        .unwrap();
    let c = sys
        .mem
        .sessions
        .create(Some("Gamma".into()), None)
        .await
        .unwrap();
    for id in [a, b, c] {
        sys.session_manager.sessions.remove(&id);
    }

    let all = sys.mem.sessions.list_summaries().await.unwrap();
    assert_eq!(all.iter().map(|s| s.id).collect::<Vec<_>>(), vec![a, b, c]);

    let ids =
        |summaries: Vec<SessionSummary>| summaries.into_iter().map(|s| s.id).collect::<Vec<_>>();
    let found = sys.mem.sessions.find("ALPHA").await.unwrap();
    assert_eq!(ids(found), vec![a, b]);
    assert!(
        sys.session_manager.sessions.is_empty(),
        "nothing was cached"
    );

    // A cached session reports its in-memory state.
    sys.mem
        .sessions
        .update_metadata(c, Some("Gamma alpha".into()), None)
        .await
        .unwrap();
    let found = sys.mem.sessions.find("alpha").await.unwrap();
    assert_eq!(ids(found), vec![a, b, c]);
}

// ----- W9: a delete removes vectors and cannot be undone by late writes ---

// ----- Eviction and reload never lose or delete acknowledged updates -----

/// Shutdown stores queued writes (eviction keeps such sessions cached,
/// see `eviction_tests`).
#[tokio::test]
async fn shutdown_flushes_pending_writes() {
    let (sys, _dir) = system(false).await;
    let b = sys.mem.sessions.create(None, None).await.unwrap();

    let ub = sys
        .mem
        .records
        .commit(b, text_record("b"), false)
        .await
        .unwrap();
    assert_eq!(sys.flush_all(Duration::from_secs(10)).await, 0);
    let stored = sys
        .session_manager
        .load_stored(b)
        .await
        .unwrap()
        .unwrap()
        .session;
    assert!(stored.records().iter().any(|u| u.id == ub.entry_id));
}
