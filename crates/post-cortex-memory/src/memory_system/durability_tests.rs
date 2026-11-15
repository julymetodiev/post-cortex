// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Durability across a stop: queued embeddings are written by `flush_all`,
//! records left without vectors are embedded again after a restart, and the
//! removals and retirements of a failed storage write are retried. Also
//! memory systems of one process sharing an embedding engine. Message
//! vectors: `message_vector_tests`.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use uuid::Uuid;

use super::SystemConfig;
use super::backends_tests::{FlakySink, TestSystem};
use super::backends_tests::{backends, embeddings_config, request};
use super::write_path_tests::text_record;
use crate::content_vectorizer::SearchOptions;

const LIMIT: Duration = Duration::from_secs(60);

/// Whether a global search for `query` returns `entry_id`.
async fn global_search_finds(sys: &TestSystem, query: &str, entry_id: Uuid) -> bool {
    let results = sys
        .mem
        .search
        .search(
            crate::capabilities::SearchScope::Global,
            query,
            SearchOptions::default(),
        )
        .await
        .unwrap();
    results
        .iter()
        .any(|r| r.content_id == entry_id.to_string() && r.neighbour_of.is_none())
}

/// A stopped missing-vector pass ends between sessions and leaves the
/// rest for the next pass: a pass told to stop before it begins embeds
/// nothing, and the next one embeds every record.
#[tokio::test]
async fn a_stopped_missing_vector_pass_leaves_the_rest_for_the_next() {
    for backend in backends().await {
        let first = backend.start().await;
        let mut sessions = Vec::new();
        for text in [
            "Ferry timetable for the winter",
            "Tide tables of the northern bay",
        ] {
            let sid = first.mem.sessions.create(None, None).await.unwrap();
            first
                .mem
                .records
                .commit(sid, text_record(text), true)
                .await
                .unwrap();
            sessions.push(sid);
        }
        assert_eq!(first.flush_all(LIMIT).await, 0);

        let restarted = backend.start_with(embeddings_config()).await;
        let stopped = tokio_util::sync::CancellationToken::new();
        stopped.cancel();
        assert_eq!(
            restarted.vectorize_missing_records(&stopped).await.unwrap(),
            0,
            "{}: a stopped pass embedded records",
            backend.name
        );
        assert!(
            restarted
                .vectorize_missing_records(&tokio_util::sync::CancellationToken::new())
                .await
                .unwrap()
                >= sessions.len(),
            "{}: the next pass did not embed the rest",
            backend.name
        );
    }
}

/// A record written just before a stop is searchable after a restart:
/// `flush_all` waits for its queued embedding, so its vector is stored.
#[tokio::test]
async fn flush_all_writes_queued_embeddings() {
    const TITLE: &str = "Lighthouse rotation schedule";
    const TEXT: &str = "The lighthouse lamp rotates every twelve seconds after sunset";
    for backend in backends().await {
        let sys = backend.start_with(embeddings_config()).await;
        let svc = sys.mem.clone();
        let sid = sys.mem.sessions.create(None, None).await.unwrap();

        // No warm-up: the embedding waits for the model load, so it is
        // still queued when the stop begins.
        let resp = svc.records.write(request(sid, TITLE, TEXT)).await.unwrap();
        assert_eq!(sys.flush_all(LIMIT).await, 0, "{}", backend.name);

        assert!(
            backend
                .vectors()
                .count_session_embeddings(&sid.to_string())
                .await
                .unwrap()
                > 0,
            "{}: the queued embedding was not written before the stop",
            backend.name
        );

        let restarted = backend.start_with(embeddings_config()).await;
        assert!(
            global_search_finds(&restarted, TEXT, resp.entry_id).await,
            "{}: record not found after restart",
            backend.name
        );
    }
}

/// A record stored without its vector (the stop came before its embedding)
/// is embedded again after a restart, so a global search finds it.
#[tokio::test]
async fn records_without_vectors_are_embedded_after_restart() {
    const TEXT: &str = "Harbour pilots board incoming tankers at the outer buoy";
    for backend in backends().await {
        // Embeddings off: the record is stored, its vector never made.
        let first = backend.start().await;
        let sid = first.mem.sessions.create(None, None).await.unwrap();
        let entry = first
            .mem
            .records
            .commit(sid, text_record(TEXT), true)
            .await
            .unwrap();
        assert!(entry.durable);
        assert_eq!(first.flush_all(LIMIT).await, 0);
        assert_eq!(
            backend
                .vectors()
                .count_session_embeddings(&sid.to_string())
                .await
                .unwrap(),
            0
        );

        let restarted = backend.start_with(embeddings_config()).await;
        // What the memory module's start runs in the background.
        restarted
            .mem
            .admin
            .warm_up(&tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
        assert!(
            global_search_finds(&restarted, TEXT, entry.entry_id).await,
            "{}: record without a vector was not embedded after restart",
            backend.name
        );
        let multi = restarted
            .mem
            .search
            .search(
                crate::capabilities::SearchScope::Sessions(vec![sid]),
                TEXT,
                SearchOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            multi
                .iter()
                .any(|r| r.content_id == entry.entry_id.to_string()),
            "{}: multisession search",
            backend.name
        );

        // Idempotent: a second pass embeds nothing again.
        assert_eq!(
            restarted
                .vectorize_missing_records(&tokio_util::sync::CancellationToken::new())
                .await
                .unwrap(),
            0,
            "{}",
            backend.name
        );
        let stored = backend
            .vectors()
            .count_session_embeddings(&sid.to_string())
            .await
            .unwrap();
        restarted
            .mem
            .search
            .search(
                crate::capabilities::SearchScope::Global,
                TEXT,
                SearchOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            backend
                .vectors()
                .count_session_embeddings(&sid.to_string())
                .await
                .unwrap(),
            stored,
            "{}: searching again added vectors",
            backend.name
        );
    }
}

/// Removals and retirements of a failed write reach storage when the
/// write is retried, and their record events are sent then.
#[tokio::test]
async fn retry_of_a_failed_write_carries_removals_and_retirements() {
    use super::RecordEventKind;

    for backend in backends().await {
        let sink = Arc::new(FlakySink::new(&backend));
        let sys = backend.start_with_sink(sink.clone()).await;
        let sid = sys.mem.sessions.create(None, None).await.unwrap();
        let mut ids = Vec::new();
        for text in ["kept record", "forgotten record", "deleted record"] {
            let c = sys
                .mem
                .records
                .commit(sid, text_record(text), true)
                .await
                .unwrap();
            assert!(c.durable);
            ids.push(c.entry_id);
        }
        let (kept, forgotten, deleted) = (ids[0], ids[1], ids[2]);
        let mut events = sys.mem.changes.subscribe();

        sink.failing.store(true, Ordering::SeqCst);
        assert!(sys.mem.records.forget(sid, forgotten).await.unwrap());
        assert!(sys.mem.records.delete(sid, deleted).await.unwrap());
        // Both writes fail.
        let live = sys.get_session(sid).await.unwrap();
        assert!(sys.persister.persist_and_wait(&live, vec![]).await.is_err());
        assert!(sys.flush_all(Duration::from_secs(5)).await > 0);

        sink.failing.store(false, Ordering::SeqCst);
        assert_eq!(sys.flush_all(LIMIT).await, 0, "{}", backend.name);

        let stored = sys
            .session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .unwrap()
            .session;
        let row = |id: Uuid| stored.records().iter().find(|u| u.id == id);
        assert!(row(kept).is_some(), "{}", backend.name);
        assert!(
            row(deleted).is_none(),
            "{}: the deleted record is back in storage",
            backend.name
        );
        assert!(
            row(forgotten).is_some_and(|u| u.meta.retired_at.is_some()),
            "{}: the forgotten record is not retired in storage",
            backend.name
        );

        let mut seen = Vec::new();
        while let Ok(Ok(e)) = tokio::time::timeout(Duration::from_millis(200), events.recv()).await
        {
            if let Some(e) = e.record() {
                seen.push((e.entry_id, e.kind));
            }
        }
        assert!(
            seen.contains(&(forgotten, RecordEventKind::Forgotten)),
            "{}: {seen:?}",
            backend.name
        );
        assert!(
            seen.contains(&(deleted, RecordEventKind::Removed)),
            "{}: {seen:?}",
            backend.name
        );
    }
}

/// Memory systems built with one shared embedding engine all embed with
/// it (one loaded model) and each searches its own records.
#[tokio::test]
async fn systems_share_one_embedding_engine() {
    use post_cortex_embeddings::{EmbeddingConfig, LocalEmbeddingEngine};

    let engine: Arc<dyn post_cortex_ports::EmbeddingEngine> = Arc::new(
        LocalEmbeddingEngine::new(EmbeddingConfig::default())
            .await
            .unwrap(),
    );
    let texts = [
        "Glacier ice cores record atmospheric carbon over millennia",
        "Sourdough starters need feeding with flour and water daily",
    ];
    let mut systems = Vec::new();
    for text in texts {
        let dir = tempfile::tempdir().unwrap();
        let config = SystemConfig {
            ..embeddings_config()
        };
        let sys =
            crate::memory_system::backends_tests::rocksdb_system(&dir, config, Arc::clone(&engine))
                .await;
        let sid = sys.mem.sessions.create(None, None).await.unwrap();
        let entry = sys
            .mem
            .records
            .commit(sid, text_record(text), true)
            .await
            .unwrap();
        assert!(sys.vectorize_update_now(sid, entry.entry_id).await.unwrap() > 0);
        let vectorizer = sys.ensure_vectorizer_initialized().await.unwrap();
        assert!(
            Arc::ptr_eq(vectorizer.embedding_engine(), &engine),
            "the system loaded its own engine"
        );
        systems.push((sys, sid, entry.entry_id, dir));
    }
    for (i, (sys, sid, entry_id, _)) in systems.iter().enumerate() {
        let results = sys
            .mem
            .search
            .search(
                crate::capabilities::SearchScope::Session(*sid),
                texts[i],
                SearchOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            results.first().map(|r| r.content_id.clone()),
            Some(entry_id.to_string()),
            "system {i} finds its record first"
        );
        let other = &systems[1 - i].2;
        assert!(
            !results.iter().any(|r| r.content_id == other.to_string()),
            "system {i} sees only its own records"
        );
    }

    // An engine of another dimension than the index's is refused rather
    // than mixed in.
    let hash = Arc::new(post_cortex_embeddings::test_support::hash_engine(1024));
    let dir = tempfile::tempdir().unwrap();
    let sys =
        crate::memory_system::backends_tests::rocksdb_system(&dir, embeddings_config(), hash).await;
    let err = sys.ensure_vectorizer_initialized().await.err().unwrap();
    assert!(
        matches!(
            err,
            crate::memory_system::RuntimeError::DimensionMismatch { engine: 1024, .. }
        ),
        "{err}"
    );
}
