// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding a batch of records, on every backend: one vector store write
//! for the records, one for the entities they name, one session persist.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use uuid::Uuid;

use super::backends_tests::hash_engine;
use super::backends_tests::{Backend, TestSystem, backends, embeddings_config};
use super::remote_tests::Calls;
use super::remote_tests::remote_system;
use super::write_path_tests::update_metadata;

const RECORDS: usize = 20;

#[tokio::test]
async fn a_batch_persists_each_session_once() {
    for backend in backends().await {
        let (memory, calls) =
            remote_system(&backend, embeddings_config(), hash_engine(), Duration::ZERO).await;
        let memory = TestSystem::new(memory);
        let sid = memory.mem.sessions.create(None, None).await.unwrap();
        let mut ids = Vec::new();
        for i in 0..RECORDS {
            let title = format!("record {i}");
            let committed = memory
                .mem
                .records
                .commit(sid, update_metadata(&title), true)
                .await
                .unwrap();
            ids.push(committed.entry_id);
        }
        memory.ensure_vectorizer_initialized().await.unwrap();
        assert_eq!(memory.persister.flush_all(Duration::from_secs(10)).await, 0);
        let writes = calls.session_writes.load(Ordering::SeqCst);
        let vector_writes = calls.vector_writes.load(Ordering::SeqCst);

        let added = memory.vectorize_updates_now(sid, &ids).await.unwrap();
        assert_eq!(added, RECORDS, "{}", backend.name);
        assert_eq!(memory.persister.flush_all(Duration::from_secs(10)).await, 0);

        assert_eq!(
            calls.session_writes.load(Ordering::SeqCst) - writes,
            1,
            "{}: one session persist for the batch",
            backend.name
        );
        assert_eq!(
            calls.vector_writes.load(Ordering::SeqCst) - vector_writes,
            2,
            "{}: one store write for the records, one for their entities",
            backend.name
        );
        let handle = memory.get_session(sid).await.unwrap();
        assert!(
            ids.iter().all(|id| handle.vectorized().contains(id)),
            "{}",
            backend.name
        );
        let stored = memory
            .session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .unwrap();
        assert!(
            ids.iter().all(|id| stored.vectorized.contains(id)),
            "{}: the persist stores the vectorized ids",
            backend.name
        );
    }
}

/// A memory system with the hash engine on `backend`, with a session of
/// [`RECORDS`] records written and stored: (system, calls, session, ids).
async fn written(backend: &Backend) -> (TestSystem, Arc<Calls>, Uuid, Vec<Uuid>) {
    let (memory, calls) =
        remote_system(backend, embeddings_config(), hash_engine(), Duration::ZERO).await;
    let memory = TestSystem::new(memory);
    let sid = memory.mem.sessions.create(None, None).await.unwrap();
    let mut ids = Vec::new();
    for i in 0..RECORDS {
        let title = format!("record {i}");
        let committed = memory
            .mem
            .records
            .commit(sid, update_metadata(&title), true)
            .await
            .unwrap();
        ids.push(committed.entry_id);
    }
    memory.ensure_vectorizer_initialized().await.unwrap();
    assert_eq!(memory.persister.flush_all(Duration::from_secs(10)).await, 0);
    (memory, calls, sid, ids)
}

/// Entries of `ids` in the index, per record.
async fn in_memory(memory: &TestSystem, sid: Uuid, ids: &[Uuid]) -> Vec<usize> {
    let vectorizer = memory.content_vectorizer.get().unwrap();
    let mut counts = Vec::new();
    for id in ids {
        let id = id.to_string();
        let entries = vectorizer
            .index()
            .entries(
                &sid.to_string(),
                &post_cortex_ports::EntrySelect::ContentIds(vec![id.clone()]),
            )
            .await
            .unwrap();
        counts.push(entries.iter().filter(|e| e.meta.id == id).count());
    }
    counts
}

/// Embedding records again (a retry after their marks were lost, or two
/// jobs racing for the same records) leaves one vector per record, in
/// memory and in storage. Before, `insert_unindexed` appended: the check
/// "already vectorized?" ran before encoding, so a retry or a concurrent
/// job added a second copy that took a result slot.
#[tokio::test(flavor = "multi_thread")]
async fn embedding_records_again_keeps_one_vector_each() {
    for backend in backends().await {
        let (memory, _, sid, ids) = written(&backend).await;
        let (a, b) = tokio::join!(
            memory.vectorize_updates_now(sid, &ids),
            memory.vectorize_updates_now(sid, &ids)
        );
        assert!(a.unwrap() + b.unwrap() >= RECORDS, "{}", backend.name);
        // The marks lost (e.g. the session was not written after its
        // embedding): the next job embeds the same records again.
        let handle = memory.get_session(sid).await.unwrap();
        for id in &ids {
            handle.vectorized().unmark(id);
        }
        assert_eq!(
            memory.vectorize_updates_now(sid, &ids).await.unwrap(),
            RECORDS,
            "{}",
            backend.name
        );
        assert_eq!(
            in_memory(&memory, sid, &ids).await,
            vec![1; RECORDS],
            "{}",
            backend.name
        );
        let vectorizer = memory.content_vectorizer.get().unwrap();
        assert_eq!(
            backend
                .vectors()
                .count_session_embeddings(&sid.to_string())
                .await
                .unwrap(),
            vectorizer.count_session_embeddings(sid).await.unwrap(),
            "{}: storage and memory hold the same vectors",
            backend.name
        );
    }
}

/// Undoing the vectorization of a session deleted mid-batch removes the
/// batch's persisted vectors in one store call (it was one call per
/// record, part A follow-up).
#[tokio::test(flavor = "multi_thread")]
async fn undo_of_a_batch_is_one_delete() {
    for backend in backends().await {
        let (memory, calls, sid, ids) = written(&backend).await;
        memory.vectorize_updates_now(sid, &ids).await.unwrap();
        let vectorizer = memory.content_vectorizer.get().unwrap();
        let deletes = calls.vector_deletes.load(Ordering::SeqCst);
        super::update_vectors::undo_vectorization(vectorizer, sid, &ids).await;
        assert_eq!(
            calls.vector_deletes.load(Ordering::SeqCst) - deletes,
            1,
            "{}",
            backend.name
        );
        assert_eq!(
            in_memory(&memory, sid, &ids).await,
            vec![0; RECORDS],
            "{}",
            backend.name
        );
        let stored = backend
            .vectors()
            .get_session_vectors(&sid.to_string())
            .await
            .unwrap();
        assert!(
            stored
                .iter()
                .all(|(_, m)| !ids.iter().any(|id| m.id == id.to_string())),
            "{}",
            backend.name
        );
    }
}
