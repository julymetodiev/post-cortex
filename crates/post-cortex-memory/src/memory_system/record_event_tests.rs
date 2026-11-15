// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Record events: one per write, forget and purge once stored, in order,
//! and a lagging receiver is told what it lost.

use super::write_path_tests::{system, text_record, update_metadata};
use std::time::Duration;
use uuid::Uuid;

/// Next record event, failing the test after a few seconds.
async fn next_event(
    rx: &mut tokio::sync::broadcast::Receiver<super::Change>,
) -> super::RecordEvent {
    *tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a record event")
        .expect("no lag")
        .record()
        .expect("a record change")
}

#[tokio::test]
async fn record_events_follow_writes_forgets_and_purges_on_every_backend() {
    use super::RecordEventKind::{Forgotten, Removed, Written};

    for backend in super::backends_tests::backends().await {
        let sys = backend.start().await;
        let sid = sys.mem.sessions.create(None, None).await.unwrap();
        let mut rx = sys.mem.changes.subscribe();

        let id: Uuid = sys
            .mem
            .records
            .commit(sid, update_metadata("watched"), false)
            .await
            .unwrap()
            .entry_id;
        let written = next_event(&mut rx).await;
        assert_eq!(
            (written.session_id, written.entry_id, written.kind),
            (sid, id, Written),
            "{}",
            backend.name
        );
        // Announced once storage holds it.
        let stored = sys
            .session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .unwrap()
            .session;
        assert!(
            stored.records().iter().any(|u| u.id == id),
            "{}",
            backend.name
        );
        let update = stored.records().iter().find(|u| u.id == id).unwrap();
        assert_eq!(written.timestamp, update.timestamp, "{}", backend.name);

        assert!(sys.mem.records.forget(sid, id).await.unwrap());
        let forgotten = next_event(&mut rx).await;
        assert_eq!(
            (forgotten.entry_id, forgotten.kind),
            (id, Forgotten),
            "{}",
            backend.name
        );

        assert!(sys.mem.records.purge(sid, id).await.unwrap());
        let purged = next_event(&mut rx).await;
        assert_eq!(
            (purged.entry_id, purged.kind),
            (id, Removed),
            "{}",
            backend.name
        );
        assert!(
            rx.try_recv().is_err(),
            "one event per change ({})",
            backend.name
        );

        // A receiver created later hears only later writes.
        let mut late = sys.mem.changes.subscribe();
        assert!(late.try_recv().is_err());
    }
}

#[tokio::test]
async fn a_lagging_record_receiver_is_told_how_many_events_it_lost() {
    let (sys, _dir) = system(false).await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();
    let mut rx = sys.mem.changes.subscribe();
    let total = crate::memory_system::change_log::CHANGE_CAPACITY + 10;
    for i in 0..total {
        sys.mem
            .records
            .commit(sid, text_record(&format!("u{i}")), true)
            .await
            .unwrap();
    }
    match rx.recv().await {
        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => assert_eq!(n, 10),
        other => panic!("expected a lag, got {other:?}"),
    }
    let mut left = 0;
    while rx.try_recv().is_ok() {
        left += 1;
    }
    assert_eq!(left, crate::memory_system::change_log::CHANGE_CAPACITY);
}
