// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// What a request with `updates` and `removed` changes.
fn changes(updates: Vec<ContextUpdate>, removed: Vec<Uuid>) -> SessionChanges {
    SessionChanges {
        updates,
        removed,
        ..Default::default()
    }
}
use crate::memory_system::ChangeLog;
use crate::memory_system::record_events::RecordEventKind;
use async_trait::async_trait;
use post_cortex_core::session::active_session::ActiveSession;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Sink that records every write and takes a while doing it, so
/// requests pile up behind a running write.
#[derive(Default)]
struct RecordingSink {
    /// (session name, update rows, removed rows) per blob write.
    writes: parking_lot::Mutex<Vec<(Option<String>, usize, usize)>>,
    deletes: AtomicUsize,
    running: AtomicUsize,
    overlapped: AtomicUsize,
    fail: AtomicBool,
}

#[async_trait]
impl PersistSink for RecordingSink {
    async fn write(
        &self,
        session: &ActiveSession,
        changes: SessionChanges,
    ) -> Result<(), PortError> {
        if self.running.fetch_add(1, Ordering::SeqCst) > 0 {
            self.overlapped.fetch_add(1, Ordering::SeqCst);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        self.running.fetch_sub(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(PortError::Unavailable("storage down".to_string()));
        }
        self.writes
            .lock()
            .push((session.name(), changes.updates.len(), changes.removed.len()));
        Ok(())
    }

    async fn delete(&self, _session_id: Uuid) -> Result<bool, PortError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
}

fn handle(id: Uuid) -> SessionHandle {
    Arc::new(CachedSession::new(
        ActiveSession::new(id, Some("v0".into()), None, chrono::Utc::now()),
        [],
    ))
}

fn rename(handle: &SessionHandle, name: &str) {
    let mut next = (**handle.load()).clone();
    next.set_name(Some(name.to_string()), chrono::Utc::now());
    handle.store(Arc::new(next));
}

fn update() -> ContextUpdate {
    use post_cortex_core::core::context_update::{UpdateContent, UpdateType};
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: "t".into(),
            description: "d".into(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    }
}

/// Persist `h` and wait for the write.
async fn flush(persister: &Arc<SessionPersister>, id: Uuid, h: &SessionHandle) -> Outcome {
    let (tx, rx) = oneshot::channel();
    persister.enqueue(id, h, changes(vec![], vec![]), Some(tx));
    rx.await.unwrap()
}

/// Let the writer task start its (slow) write.
async fn let_writer_start() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

#[tokio::test]
async fn queued_persists_coalesce_and_the_newest_version_lands_last() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);
    persister.register(id, &h);

    const K: usize = 20;
    rename(&h, "v1");
    persister.enqueue(id, &h, changes(vec![update()], vec![]), None);
    let_writer_start().await;
    for i in 2..=K {
        rename(&h, &format!("v{i}"));
        persister.enqueue(id, &h, changes(vec![update()], vec![]), None);
    }
    flush(&persister, id, &h).await.unwrap();

    let writes = sink.writes.lock().clone();
    // The first request starts a write; the other K-1 (and the flush)
    // queue behind it and merge into a single second write.
    assert_eq!(writes.len(), 2, "expected 2 coalesced writes: {writes:?}");
    assert_eq!(writes[0].1, 1);
    assert_eq!(writes[1].1, K - 1, "every update row is written once");
    assert_eq!(
        writes[1].0.as_deref(),
        Some(format!("v{K}").as_str()),
        "the newest version is written last"
    );
    assert_eq!(
        sink.overlapped.load(Ordering::SeqCst),
        0,
        "writes never overlap"
    );
    assert!(persister.slots.is_empty(), "idle slots are unregistered");
}

#[tokio::test]
async fn removal_cancels_a_queued_row_and_reaches_the_sink() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);

    persister.enqueue(id, &h, changes(vec![update()], vec![]), None);
    let_writer_start().await;
    let doomed = update();
    persister.enqueue(id, &h, changes(vec![doomed.clone()], vec![]), None);
    persister.enqueue(id, &h, changes(vec![], vec![doomed.id]), None);
    flush(&persister, id, &h).await.unwrap();

    let writes = sink.writes.lock().clone();
    assert_eq!(writes[1].1, 0, "the removed update's row is not written");
    assert_eq!(writes[1].2, 1, "its removal is passed down");
}

#[tokio::test]
async fn delete_drops_queued_persists_and_blocks_late_ones() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);
    persister.register(id, &h);

    persister.enqueue(id, &h, changes(vec![update()], vec![]), None);
    let_writer_start().await;
    let (tx, queued) = oneshot::channel();
    persister.enqueue(id, &h, changes(vec![update()], vec![]), Some(tx));

    persister.delete(id, None).await.unwrap();
    assert_eq!(queued.await.unwrap(), Err(PersistError::SessionDeleted));
    assert!(persister.is_deleted(id, &h));

    // A task still holding the handle cannot write the session back.
    assert_eq!(
        flush(&persister, id, &h).await,
        Err(PersistError::SessionDeleted)
    );
    assert_eq!(
        sink.writes.lock().len(),
        1,
        "only the running write happened"
    );
    assert_eq!(sink.deletes.load(Ordering::SeqCst), 1);

    // A session re-created under the same id afterwards is not affected.
    let fresh = handle(id);
    persister.register(id, &fresh);
    assert!(!persister.is_deleted(id, &fresh));
    flush(&persister, id, &fresh).await.unwrap();
    assert_eq!(sink.writes.lock().len(), 2);
}

#[tokio::test]
async fn request_of_a_recreated_session_during_a_delete_is_written() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let old = handle(id);
    persister.register(id, &old);

    persister.enqueue(id, &old, changes(vec![update()], vec![]), None);
    let_writer_start().await;
    let deleting = {
        let persister = Arc::clone(&persister);
        tokio::spawn(async move { persister.delete(id, None).await })
    };
    tokio::task::yield_now().await; // delete waits for the running write

    let fresh = handle(id);
    rename(&fresh, "recreated");
    persister.register(id, &fresh);
    let (tx, rx) = oneshot::channel();
    persister.enqueue(id, &fresh, changes(vec![update()], vec![]), Some(tx));

    deleting.await.unwrap().unwrap();
    assert_eq!(rx.await.unwrap(), Ok(()), "not reported as deleted");
    assert_eq!(
        sink.writes.lock().last().unwrap().0.as_deref(),
        Some("recreated"),
        "written after the delete"
    );
}

#[tokio::test]
async fn unregistered_live_handle_is_tombstoned_on_delete() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);

    persister.delete(id, Some(&h)).await.unwrap();
    assert_eq!(
        flush(&persister, id, &h).await,
        Err(PersistError::SessionDeleted)
    );
    assert!(sink.writes.lock().is_empty());
}

#[tokio::test]
async fn superseded_handle_writes_the_newest_or_nothing() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let old = handle(id);
    persister.register(id, &old);
    let new = handle(id);
    rename(&new, "reloaded");
    persister.register(id, &new);

    // The evicted copy cannot regress storage to its older state.
    flush(&persister, id, &old).await.unwrap();
    assert_eq!(
        sink.writes.lock().last().unwrap().0.as_deref(),
        Some("reloaded")
    );

    drop(new);
    assert_eq!(
        flush(&persister, id, &old).await,
        Err(PersistError::HandleSuperseded)
    );
    assert_eq!(sink.writes.lock().len(), 1);
}

#[tokio::test]
async fn failed_writes_are_tracked_and_retried_by_flush_all() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);

    sink.fail.store(true, Ordering::SeqCst);
    assert!(flush(&persister, id, &h).await.is_err());
    assert!(
        persister.has_unflushed(id),
        "a failed write stays unflushed"
    );
    assert_eq!(persister.flush_all(Duration::from_secs(5)).await, 1);

    sink.fail.store(false, Ordering::SeqCst);
    assert_eq!(persister.flush_all(Duration::from_secs(5)).await, 0);
    assert!(!persister.has_unflushed(id));
    assert_eq!(sink.writes.lock().len(), 1);
}

#[tokio::test]
async fn retry_of_a_failed_write_carries_its_removals_and_retirements() {
    let sink = Arc::new(RecordingSink::default());
    let log = ChangeLog::new();
    let persister = SessionPersister::new(sink.clone(), log.clone());
    let mut rx = log.subscribe();
    let id = Uuid::new_v4();
    let h = handle(id);

    sink.fail.store(true, Ordering::SeqCst);
    let mut retired = update();
    retired.meta.retired_at = Some(chrono::Utc::now());
    let gone = Uuid::new_v4();
    let (tx, done) = oneshot::channel();
    persister.enqueue(id, &h, changes(vec![retired.clone()], vec![gone]), Some(tx));
    assert!(done.await.unwrap().is_err());

    // The next write of the session (here a bare persist) redoes them.
    sink.fail.store(false, Ordering::SeqCst);
    flush(&persister, id, &h).await.unwrap();
    assert_eq!(
        sink.writes.lock().last().map(|w| (w.1, w.2)),
        Some((1, 1)),
        "the retry writes the retired row and deletes the removed one"
    );
    let events: Vec<(Uuid, RecordEventKind)> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|c| c.record().copied())
        .map(|e| (e.entry_id, e.kind))
        .collect();
    assert_eq!(
        events,
        [
            (retired.id, RecordEventKind::Forgotten),
            (gone, RecordEventKind::Removed),
        ]
    );

    // `flush_all` retries them too.
    sink.fail.store(true, Ordering::SeqCst);
    let gone = Uuid::new_v4();
    persister.enqueue(id, &h, changes(vec![], vec![gone]), None);
    assert!(persister.flush_all(Duration::from_secs(5)).await > 0);
    sink.fail.store(false, Ordering::SeqCst);
    assert_eq!(persister.flush_all(Duration::from_secs(5)).await, 0);
    assert_eq!(sink.writes.lock().last().map(|w| w.2), Some(1));
    assert_eq!(
        rx.try_recv()
            .ok()
            .and_then(|c| c.record().copied())
            .map(|e| (e.entry_id, e.kind)),
        Some((gone, RecordEventKind::Removed))
    );
}

#[tokio::test]
async fn wait_idle_returns_after_queued_writes() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);

    persister.enqueue(id, &h, changes(vec![update()], vec![]), None);
    persister.wait_idle(id).await.unwrap();
    assert_eq!(sink.writes.lock().len(), 1);
    persister.wait_idle(Uuid::new_v4()).await.unwrap(); // nothing queued: no wait
}

#[tokio::test]
async fn finished_writes_announce_their_records_and_failed_ones_wait() {
    let sink = Arc::new(RecordingSink::default());
    let log = ChangeLog::new();
    let persister = SessionPersister::new(sink.clone(), log.clone());
    let mut rx = log.subscribe();
    let id = Uuid::new_v4();
    let h = handle(id);

    // A failed write announces nothing yet.
    sink.fail.store(true, Ordering::SeqCst);
    let first = update();
    persister.enqueue(id, &h, changes(vec![first.clone()], vec![]), None);
    assert!(flush(&persister, id, &h).await.is_err());
    assert!(rx.try_recv().is_err());

    // The next successful write announces it first, then its own.
    sink.fail.store(false, Ordering::SeqCst);
    let mut retired = update();
    retired.meta.retired_at = Some(chrono::Utc::now());
    let gone = Uuid::new_v4();
    let (tx, done) = oneshot::channel();
    persister.enqueue(id, &h, changes(vec![retired.clone()], vec![gone]), Some(tx));
    done.await.unwrap().unwrap();

    let events: Vec<(Uuid, RecordEventKind)> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|c| c.record().copied())
        .inspect(|e| assert_eq!(e.session_id, id))
        .map(|e| (e.entry_id, e.kind))
        .collect();
    assert_eq!(
        events,
        [
            (first.id, RecordEventKind::Written),
            (retired.id, RecordEventKind::Forgotten),
            (gone, RecordEventKind::Removed),
        ]
    );
}

/// Sink whose write outlasts the caller-side deadline the storage actor
/// put on a waiting persist (30 s, `OperationType::Medium`).
struct SlowSink;

#[async_trait]
impl PersistSink for SlowSink {
    async fn write(
        &self,
        _session: &ActiveSession,
        _changes: SessionChanges,
    ) -> Result<(), PortError> {
        tokio::time::sleep(Duration::from_secs(45)).await;
        Ok(())
    }

    async fn delete(&self, _session_id: Uuid) -> Result<bool, PortError> {
        Ok(true)
    }
}

/// A waiting persist reports how the write ended, however long it took.
/// The storage actor's handle gave up after 30 s and reported "timed out"
/// for a write that still landed (review W3 item 17).
#[tokio::test(start_paused = true)]
async fn a_slow_write_is_reported_as_written() {
    let persister = SessionPersister::new(Arc::new(SlowSink), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);
    persister.register(id, &h);
    assert_eq!(persister.persist_and_wait(&h, vec![update()]).await, Ok(()));
}

/// A write that does not land is reported to `wait_idle`, so a load that
/// follows it cannot read an older stored session as if it were current
/// (audit 2026-10-22: `wait_idle` dropped the outcome).
#[tokio::test]
async fn wait_idle_reports_a_write_that_did_not_land() {
    let sink = Arc::new(RecordingSink::default());
    let persister = SessionPersister::new(sink.clone(), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);

    sink.fail.store(true, Ordering::SeqCst);
    persister.enqueue(id, &h, changes(vec![update()], vec![]), None);
    assert!(matches!(
        persister.wait_idle(id).await,
        Err(PersistError::Write(_))
    ));

    sink.fail.store(false, Ordering::SeqCst);
    persister.wait_idle(id).await.unwrap();
    assert!(!persister.has_unflushed(id));
}

/// Sink whose first write panics, like an adapter bug would.
#[derive(Default)]
struct PanickingSink {
    panicked: AtomicBool,
}

#[async_trait]
impl PersistSink for PanickingSink {
    async fn write(
        &self,
        _session: &ActiveSession,
        _changes: SessionChanges,
    ) -> Result<(), PortError> {
        if !self.panicked.swap(true, Ordering::SeqCst) {
            panic!("adapter bug");
        }
        Ok(())
    }

    async fn delete(&self, _session_id: Uuid) -> Result<bool, PortError> {
        Ok(true)
    }
}

/// A panicking storage adapter fails that write; its waiters are answered
/// and the session's later writes still drain (audit 2026-10-22: a panic in
/// the writer task left the slot registered with nobody draining it, so
/// every later waiter of the session hung).
#[tokio::test]
async fn an_adapter_panic_fails_the_write_and_the_slot_keeps_draining() {
    let persister = SessionPersister::new(Arc::new(PanickingSink::default()), ChangeLog::new());
    let id = Uuid::new_v4();
    let h = handle(id);

    assert_eq!(flush(&persister, id, &h).await, Err(PersistError::Panicked));
    flush(&persister, id, &h).await.unwrap();
    persister.wait_idle(id).await.unwrap();
}
