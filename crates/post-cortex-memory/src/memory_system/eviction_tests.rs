// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The session cache's capacity holds, and eviction never loses work: a
//! session with writes not yet stored or derived work queued stays cached
//! until they are done.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::SystemConfig;
use super::backends_tests::{FlakySink, backends};
use super::write_path_tests::text_record;

fn small_cache() -> SystemConfig {
    SystemConfig {
        enable_embeddings: false,
        cache_capacity: 1,
        ..Default::default()
    }
}

/// A session whose embedding or graph merge is queued is not evicted (the
/// cache used to evict any entry when it filled up).
#[tokio::test]
async fn sessions_with_derived_work_stay_cached() {
    for backend in backends().await {
        let sys = backend.start_with(small_cache()).await;
        let a = sys.mem.sessions.create(None, None).await.unwrap();
        let job = sys.derived_work.begin(a);
        let b = sys.mem.sessions.create(None, None).await.unwrap();
        assert!(
            sys.session_manager.sessions.contains(&a),
            "{}",
            backend.name
        );
        assert!(sys.session_manager.sessions.contains(&b));

        drop(job);
        let c = sys.mem.sessions.create(None, None).await.unwrap();
        assert!(
            !sys.session_manager.sessions.contains(&a),
            "{}",
            backend.name
        );
        assert_eq!(sys.session_manager.sessions.len(), 1);
        assert!(sys.session_manager.sessions.contains(&c));
    }
}

/// A session whose last write failed is not evicted, so its committed
/// records are not lost; once a retry stores them it can go.
#[tokio::test]
async fn sessions_with_unstored_writes_stay_cached() {
    for backend in backends().await {
        let sink = Arc::new(FlakySink::new(&backend));
        let sys = backend
            .start_with_sink_and(small_cache(), sink.clone())
            .await;
        let a = sys.mem.sessions.create(None, None).await.unwrap();
        sink.failing.store(true, Ordering::SeqCst);
        let write = sys
            .mem
            .records
            .commit(a, text_record("kept in memory"), true)
            .await
            .unwrap();
        assert!(!write.durable);

        let b = sys.mem.sessions.create(None, None).await;
        assert!(
            sys.session_manager.sessions.contains(&a),
            "{}",
            backend.name
        );

        sink.failing.store(false, Ordering::SeqCst);
        assert_eq!(sys.flush_all(Duration::from_secs(10)).await, 0);
        let c = sys.mem.sessions.create(None, None).await.unwrap();
        assert!(
            !sys.session_manager.sessions.contains(&a),
            "{}",
            backend.name
        );
        assert!(sys.session_manager.sessions.contains(&c));
        let stored = sys
            .session_manager
            .load_stored(a)
            .await
            .unwrap()
            .unwrap()
            .session;
        assert!(stored.record(&write.entry_id).is_some(), "{}", backend.name);
        drop(b);
    }
}

/// What was derived from an evicted session goes with it.
#[tokio::test]
async fn derived_state_is_evicted_with_its_session() {
    for backend in backends().await {
        let sys = backend.start_with(small_cache()).await;
        let a = sys.mem.sessions.create(None, None).await.unwrap();
        let session = sys.get_session(a).await.unwrap().load_full();
        sys.session_manager.validity.of(&session);
        drop(session);
        assert_eq!(sys.session_manager.validity.len(), 1);

        sys.mem.sessions.create(None, None).await.unwrap();
        assert!(!sys.session_manager.sessions.contains(&a));
        assert_eq!(sys.session_manager.validity.len(), 0, "{}", backend.name);
    }
}
