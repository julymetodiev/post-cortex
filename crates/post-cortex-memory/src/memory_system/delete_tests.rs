// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Deletes: vectors go with the session, and no late work, held handle,
//! racing load or stale copy brings a deleted session back.

use super::write_path_tests::{system, text_record, update_metadata};
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn delete_removes_vectors_and_late_work_cannot_resurrect_the_session() {
    let (sys, _dir) = system(true).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    let first = sys
        .mem
        .records
        .commit(sid, update_metadata("first"), true)
        .await
        .unwrap();
    let second = sys
        .mem
        .records
        .commit(sid, update_metadata("second"), true)
        .await
        .unwrap();
    assert!(sys.vectorize_update_now(sid, first.entry_id).await.unwrap() > 0);

    let vectorizer = sys.ensure_vectorizer_initialized().await.unwrap();
    assert!(vectorizer.count_session_embeddings(sid).await.unwrap() > 0);
    assert!(
        sys.stored()
            .count_session_embeddings(&sid.to_string())
            .await
            .unwrap()
            > 0
    );

    let old_handle = sys.get_session(sid).await.unwrap();
    assert!(sys.mem.sessions.delete(sid).await.unwrap());

    assert_eq!(vectorizer.count_session_embeddings(sid).await.unwrap(), 0);
    assert_eq!(
        sys.stored()
            .count_session_embeddings(&sid.to_string())
            .await
            .unwrap(),
        0
    );

    // Work still holding the old handle: a vectorization that finishes
    // after the delete, and a persist.
    let late = sys
        .vectorize_and_commit(
            &vectorizer,
            sid,
            &old_handle,
            super::embeddings::VectorizeTarget::Updates(&[second.entry_id]),
        )
        .await
        .unwrap();
    assert_eq!(late, 0, "a vectorization of a deleted session adds nothing");
    let persisted = sys.persister.persist_and_wait(&old_handle, vec![]).await;
    assert!(
        persisted.is_err(),
        "persist of a deleted session must be refused"
    );

    assert!(
        old_handle.vectorized().contains(&second.entry_id),
        "the late vectorization ran"
    );
    assert_eq!(vectorizer.count_session_embeddings(sid).await.unwrap(), 0);
    assert_eq!(
        sys.stored()
            .count_session_embeddings(&sid.to_string())
            .await
            .unwrap(),
        0
    );
    assert!(
        sys.session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(sys.get_session(sid).await.is_err());
}

#[tokio::test]
async fn a_handle_held_outside_the_cache_cannot_resurrect_a_deleted_session() {
    let (sys, _dir) = system(false).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    // A worker loaded the session, then it left the cache (live = None at
    // delete time).
    let held = sys.get_session(sid).await.unwrap();
    sys.session_manager.sessions.remove(&sid);
    assert!(sys.mem.sessions.delete(sid).await.unwrap());

    let mut next = (**held.load()).clone();
    next.set_name(Some("ghost".into()), chrono::Utc::now());
    held.store(Arc::new(next));
    assert!(sys.persister.persist_and_wait(&held, vec![]).await.is_err());
    assert!(
        sys.session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn loads_racing_a_delete_never_bring_the_session_back() {
    let (sys, _dir) = system(false).await;
    for round in 0..20 {
        let sid = sys.mem.sessions.create(None, None).await.unwrap();
        sys.mem
            .records
            .commit(sid, text_record(&format!("round {round}")), false)
            .await
            .unwrap();
        let live = sys.get_session(sid).await.unwrap();
        sys.persister.persist_and_wait(&live, vec![]).await.unwrap();
        sys.session_manager.sessions.remove(&sid);

        // Pipeline-worker-like tasks: load (cache miss), modify, persist.
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let sys = sys.clone();
                tokio::spawn(async move {
                    if let Ok(Some(h)) = sys.session_manager.get_session(sid).await {
                        let mut next = (**h.load()).clone();
                        next.set_name(Some("worker".into()), chrono::Utc::now());
                        h.store(Arc::new(next));
                        let _ = sys.persister.persist_and_wait(&h, vec![]).await;
                    }
                })
            })
            .collect();
        tokio::task::yield_now().await;
        assert!(sys.mem.sessions.delete(sid).await.unwrap());
        for w in workers {
            w.await.unwrap();
        }

        assert!(
            sys.session_manager
                .load_stored(sid)
                .await
                .unwrap()
                .is_none(),
            "round {round}: session written back after its delete"
        );
    }
}

#[tokio::test]
async fn reload_after_eviction_sees_queued_writes_and_stale_copies_delete_nothing() {
    let (sys, _dir) = system(false).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();

    // Queued, not yet written; then the cache drops the handle (capacity
    // eviction does not flush).
    let first = sys
        .mem
        .records
        .commit(sid, update_metadata("a"), false)
        .await
        .unwrap();
    let stale = sys.get_session(sid).await.unwrap();
    sys.session_manager.sessions.remove(&sid);

    let reloaded = sys.get_session(sid).await.unwrap();
    assert!(!Arc::ptr_eq(&stale, &reloaded));
    assert!(
        reloaded
            .load()
            .records()
            .iter()
            .any(|u| u.id == first.entry_id),
        "the reload waited for the evicted copy's queued write"
    );

    // A new update on the reloaded copy, then a late persist of the stale
    // copy (which lacks it): the update must survive.
    let second = sys
        .mem
        .records
        .commit(sid, text_record("second"), true)
        .await
        .unwrap();
    assert!(second.durable);
    let _ = sys.persister.persist_and_wait(&stale, vec![]).await;
    let stored = sys
        .session_manager
        .load_stored(sid)
        .await
        .unwrap()
        .unwrap()
        .session;
    let ids: Vec<Uuid> = stored.records().iter().map(|u| u.id).collect();
    assert!(ids.contains(&first.entry_id) && ids.contains(&second.entry_id));

    // Explicit removal still deletes.
    assert!(sys.mem.records.delete(sid, first.entry_id).await.unwrap());
    let live = sys.get_session(sid).await.unwrap();
    sys.persister.persist_and_wait(&live, vec![]).await.unwrap();
    let stored = sys
        .session_manager
        .load_stored(sid)
        .await
        .unwrap()
        .unwrap()
        .session;
    assert!(!stored.records().iter().any(|u| u.id == first.entry_id));
}
