//! Coalescing, per-session ordered session persistence.
//!
//! Every write path used to send a full session snapshot to the storage
//! actor, which spawned one task per snapshot. Nothing coalesced them (K
//! updates meant K full blob writes) and task start order is not send
//! order, so an older snapshot could land after a newer one.
//!
//! [`SessionPersister`] keeps one slot per session instead. A persist
//! request names the session's live `ArcSwap` and the update rows to write
//! (and to remove); requests that queue up while a write is in flight merge
//! into the slot. One writer task per session drains it, loading the newest
//! snapshot from the `ArcSwap` at write time, so K queued requests cost one
//! blob write (carrying all K update rows) and the last write always holds
//! the latest state.
//!
//! Handles are registered per session with a generation number. A handle
//! that a newer one superseded (the session was evicted and loaded again)
//! never writes its own, older snapshot: its requests go to the newest live
//! handle or are dropped. Deleting a session retires every handle
//! registered so far and drains the slot under the per-session write lock,
//! so a persist still in flight for it (a pipeline worker, a vectorization
//! task) cannot write the session back.

use std::sync::{Arc, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::ActiveSession;

/// Live, shared handle to a session's current version.
pub type SessionHandle = Arc<ArcSwap<ActiveSession>>;

/// Error reported to waiters of a persist whose session was deleted.
pub(crate) const SESSION_DELETED: &str = "session was deleted";
/// Error reported to waiters of a persist through a superseded handle.
pub(crate) const HANDLE_SUPERSEDED: &str = "session handle was superseded by a newer load";

/// Where [`SessionPersister`] writes. Implemented for the storage trait
/// object; a seam so ordering and coalescing can be tested.
#[async_trait]
pub(crate) trait PersistSink: Send + Sync {
    /// Write the session blob plus `updates` (and any update rows the
    /// backend has not stored yet), and delete the `removed` updates' rows.
    async fn write(
        &self,
        session: &ActiveSession,
        updates: Vec<ContextUpdate>,
        removed: Vec<Uuid>,
    ) -> anyhow::Result<()>;
    /// Delete the session and everything stored for it.
    async fn delete(&self, session_id: Uuid) -> anyhow::Result<()>;
}

/// [`PersistSink`] over the storage backend.
pub(crate) struct StorageSink(pub Arc<dyn post_cortex_storage::traits::GraphStorage>);

#[async_trait]
impl PersistSink for StorageSink {
    async fn write(
        &self,
        session: &ActiveSession,
        updates: Vec<ContextUpdate>,
        removed: Vec<Uuid>,
    ) -> anyhow::Result<()> {
        self.0
            .save_session_with_changes(session, session.id(), updates, removed)
            .await
    }

    async fn delete(&self, session_id: Uuid) -> anyhow::Result<()> {
        self.0.delete_session(session_id).await
    }
}

type Waiter = oneshot::Sender<Result<(), String>>;

/// Work queued for one session.
#[derive(Default)]
struct Pending {
    /// Session to snapshot at write time; `None` when only waiters queued.
    source: Option<SessionHandle>,
    /// Update rows to write with the next blob.
    updates: Vec<ContextUpdate>,
    /// Updates whose rows the next write deletes.
    removed: Vec<Uuid>,
    /// Notified once a write covering their request finished.
    waiters: Vec<Waiter>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.source.is_none()
            && self.updates.is_empty()
            && self.removed.is_empty()
            && self.waiters.is_empty()
    }

    fn fail(self, error: &str) {
        for waiter in self.waiters {
            let _ = waiter.send(Err(error.to_string()));
        }
    }
}

/// One session's queue and write lock.
struct Slot {
    /// Held for the duration of every storage write of this session, so
    /// writes (and deletes) of one session never overlap.
    write_lock: tokio::sync::Mutex<()>,
    pending: parking_lot::Mutex<Pending>,
    /// Outcome of the last write, reported to waiters that queued while
    /// that write was already running with nothing new to add.
    last_result: parking_lot::Mutex<Result<(), String>>,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            write_lock: tokio::sync::Mutex::new(()),
            pending: parking_lot::Mutex::new(Pending::default()),
            last_result: parking_lot::Mutex::new(Ok(())),
        }
    }
}

/// Handles registered for one session id.
#[derive(Default)]
struct Lineage {
    /// Live registered handles with their generation.
    members: Vec<(Weak<ArcSwap<ActiveSession>>, u64)>,
    /// Generation of the next registered handle.
    next: u64,
    /// Handles below this generation belong to a deleted incarnation.
    deleted_below: u64,
}

impl Lineage {
    fn prune(&mut self) {
        self.members.retain(|(w, _)| w.strong_count() > 0);
    }

    fn generation_of(&self, handle: &SessionHandle) -> Option<u64> {
        self.members
            .iter()
            .find(|(w, _)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(handle)))
            .map(|(_, g)| *g)
    }

    fn newest_alive(&self) -> Option<(SessionHandle, u64)> {
        self.members
            .iter()
            .filter_map(|(w, g)| w.upgrade().map(|h| (h, *g)))
            .max_by_key(|(_, g)| *g)
    }
}

/// How a persist through a given handle is treated.
enum Standing {
    /// The handle is the session's current one (or was never registered).
    Current,
    /// A newer handle replaced this one; write through it instead, or drop
    /// the request when it is gone too.
    Superseded(Option<SessionHandle>),
    /// The session was deleted after this handle was created.
    Deleted,
}

/// Coalescing per-session writer. See the module docs.
pub(crate) struct SessionPersister {
    sink: Arc<dyn PersistSink>,
    slots: DashMap<Uuid, Arc<Slot>>,
    /// Unregistered session handles deleted while something may hold them.
    tombstones: DashMap<Uuid, Vec<Weak<ArcSwap<ActiveSession>>>>,
    lineages: DashMap<Uuid, Lineage>,
    /// Sessions whose last write failed, with the handle to retry from.
    failed: DashMap<Uuid, Weak<ArcSwap<ActiveSession>>>,
}

impl SessionPersister {
    pub(crate) fn new(sink: Arc<dyn PersistSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            slots: DashMap::new(),
            tombstones: DashMap::new(),
            lineages: DashMap::new(),
            failed: DashMap::new(),
        })
    }

    /// Register a freshly loaded or created handle as the session's
    /// current one; older registered handles become superseded.
    pub(crate) fn register(&self, session_id: Uuid, handle: &SessionHandle) {
        let mut lineage = self.lineages.entry(session_id).or_default();
        lineage.prune();
        let generation = lineage.next;
        lineage.next += 1;
        lineage.members.push((Arc::downgrade(handle), generation));
    }

    fn standing(&self, session_id: Uuid, handle: &SessionHandle) -> Standing {
        if self.is_tombstoned(session_id, handle) {
            return Standing::Deleted;
        }
        let Some(lineage) = self.lineages.get(&session_id) else {
            return Standing::Current;
        };
        let Some(generation) = lineage.generation_of(handle) else {
            return Standing::Current;
        };
        if generation < lineage.deleted_below {
            return Standing::Deleted;
        }
        if generation + 1 == lineage.next {
            return Standing::Current;
        }
        Standing::Superseded(
            lineage
                .newest_alive()
                .filter(|(_, g)| *g > generation)
                .map(|(h, _)| h),
        )
    }

    /// Whether `handle` belongs to a session that has been deleted.
    pub(crate) fn is_deleted(&self, session_id: Uuid, handle: &SessionHandle) -> bool {
        matches!(self.standing(session_id, handle), Standing::Deleted)
    }

    fn is_tombstoned(&self, session_id: Uuid, handle: &SessionHandle) -> bool {
        let Some(mut dead) = self.tombstones.get_mut(&session_id) else {
            return false;
        };
        dead.retain(|w| w.strong_count() > 0);
        let hit = dead
            .iter()
            .any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(handle)));
        let empty = dead.is_empty();
        drop(dead);
        if empty {
            self.tombstones.remove_if(&session_id, |_, v| v.is_empty());
        }
        hit
    }

    fn tombstone(&self, session_id: Uuid, handle: &SessionHandle) {
        let mut dead = self.tombstones.entry(session_id).or_default();
        dead.retain(|w| w.strong_count() > 0);
        if !dead
            .iter()
            .any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(handle)))
        {
            dead.push(Arc::downgrade(handle));
        }
    }

    /// Whether the session has writes queued or running, or its last write
    /// failed: evicting it now could lose committed data.
    pub(crate) fn has_unflushed(&self, session_id: Uuid) -> bool {
        self.slots.contains_key(&session_id) || self.failed.contains_key(&session_id)
    }

    /// Queue a write of `handle`'s latest version plus `updates`, deleting
    /// the `removed` updates' rows. With a `waiter`, it is told the outcome
    /// of the write that covers this call.
    pub(crate) fn enqueue(
        self: &Arc<Self>,
        session_id: Uuid,
        handle: &SessionHandle,
        updates: Vec<ContextUpdate>,
        removed: Vec<Uuid>,
        waiter: Option<Waiter>,
    ) {
        let source = match self.standing(session_id, handle) {
            Standing::Current => Arc::clone(handle),
            Standing::Superseded(Some(newest)) => newest,
            Standing::Superseded(None) => {
                warn!(
                    "Dropping persist of session {} through a superseded handle",
                    session_id
                );
                if let Some(w) = waiter {
                    let _ = w.send(Err(HANDLE_SUPERSEDED.to_string()));
                }
                return;
            }
            Standing::Deleted => {
                debug!("Dropping persist of deleted session {}", session_id);
                if let Some(w) = waiter {
                    let _ = w.send(Err(SESSION_DELETED.to_string()));
                }
                return;
            }
        };
        self.queue(
            session_id,
            Pending {
                source: Some(source),
                updates,
                removed,
                waiters: waiter.into_iter().collect(),
            },
        );
    }

    /// Merge `work` into the session's slot, starting its writer if needed.
    fn queue(self: &Arc<Self>, session_id: Uuid, work: Pending) {
        // The pending queue is extended while the map entry is held, and the
        // writer only unregisters an empty slot under that same entry lock,
        // so a request can never land in a slot nobody drains.
        let spawn = match self.slots.entry(session_id) {
            Entry::Occupied(slot) => {
                merge(&slot.get().pending, work);
                None
            }
            Entry::Vacant(vacant) => {
                let slot = Arc::new(Slot::default());
                merge(&slot.pending, work);
                vacant.insert(Arc::clone(&slot));
                Some(slot)
            }
        };

        if let Some(slot) = spawn {
            let this = Arc::clone(self);
            tokio::spawn(async move { this.drain(session_id, slot).await });
        }
    }

    /// A receiver told when everything queued for the session so far is
    /// written, or `None` when nothing is queued.
    fn wait_queued(&self, session_id: Uuid) -> Option<oneshot::Receiver<Result<(), String>>> {
        let slot = self.slots.get(&session_id)?;
        let (tx, rx) = oneshot::channel();
        slot.pending.lock().waiters.push(tx);
        Some(rx)
    }

    /// Wait until the session's queued writes are done (retrying a failed
    /// last write once), so a load that follows reads them.
    pub(crate) async fn wait_idle(self: &Arc<Self>, session_id: Uuid) {
        if let Some(rx) = self.wait_queued(session_id) {
            let _ = rx.await;
        }
        let failed = self.failed.get(&session_id).map(|w| w.upgrade());
        if let Some(Some(handle)) = failed {
            let (tx, rx) = oneshot::channel();
            self.enqueue(session_id, &handle, vec![], vec![], Some(tx));
            let _ = rx.await;
        }
    }

    /// Write everything queued and retry sessions whose last write failed;
    /// returns once every write finished or `limit` passed. Returns how
    /// many sessions are still not written. For graceful shutdown.
    pub(crate) async fn flush_all(self: &Arc<Self>, limit: Duration) -> usize {
        let mut receivers = Vec::new();
        let failed: Vec<(Uuid, Weak<ArcSwap<ActiveSession>>)> = self
            .failed
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect();
        for (session_id, handle) in failed {
            if let Some(handle) = handle.upgrade() {
                let (tx, rx) = oneshot::channel();
                self.enqueue(session_id, &handle, vec![], vec![], Some(tx));
                receivers.push((session_id, rx));
            } else {
                self.failed.remove(&session_id);
            }
        }
        let queued: Vec<Uuid> = self.slots.iter().map(|e| *e.key()).collect();
        receivers.extend(
            queued
                .into_iter()
                .filter_map(|id| self.wait_queued(id).map(|rx| (id, rx))),
        );

        let ids: Vec<Uuid> = receivers.iter().map(|(id, _)| *id).collect();
        let all = futures::future::join_all(receivers.into_iter().map(|(_, rx)| rx));
        let unwritten: std::collections::HashSet<Uuid> =
            match tokio::time::timeout(limit, all).await {
                Ok(results) => ids
                    .into_iter()
                    .zip(results)
                    .filter(|(_, r)| !matches!(r, Ok(Ok(()))))
                    .map(|(id, _)| id)
                    .collect(),
                Err(_) => ids.into_iter().collect(),
            };
        unwritten.len()
    }

    /// Writer task: one per registered slot, so per session.
    async fn drain(self: Arc<Self>, session_id: Uuid, slot: Arc<Slot>) {
        loop {
            {
                let _write = slot.write_lock.lock().await;
                let batch = std::mem::take(&mut *slot.pending.lock());
                let registered = self
                    .slots
                    .get(&session_id)
                    .is_some_and(|s| Arc::ptr_eq(&s, &slot));
                self.write_batch(session_id, &slot, batch, registered).await;
            }

            // Unregister once nothing is queued; checked under the entry
            // lock that `enqueue` holds while queueing.
            let removed = self.slots.remove_if(&session_id, |_, s| {
                Arc::ptr_eq(s, &slot) && s.pending.lock().is_empty()
            });
            if removed.is_some() {
                break;
            }
            let registered = self
                .slots
                .get(&session_id)
                .is_some_and(|s| Arc::ptr_eq(&s, &slot));
            if !registered && slot.pending.lock().is_empty() {
                // Drained by a delete.
                break;
            }
        }
    }

    async fn write_batch(&self, session_id: Uuid, slot: &Slot, batch: Pending, registered: bool) {
        let Pending {
            source,
            updates,
            removed,
            waiters,
        } = batch;

        let source = match source {
            // Only waiters: the write they wait for already ran.
            None if registered => Err(slot.last_result.lock().clone()),
            Some(handle) if registered => match self.standing(session_id, &handle) {
                Standing::Current => Ok(handle),
                Standing::Superseded(Some(newest)) => Ok(newest),
                Standing::Superseded(None) => Err(Err(HANDLE_SUPERSEDED.to_string())),
                Standing::Deleted => Err(Err(SESSION_DELETED.to_string())),
            },
            // The slot was retired by a delete.
            _ => Err(Err(SESSION_DELETED.to_string())),
        };
        let result = match source {
            Err(outcome) => {
                debug!("Persist of session {} skipped: {:?}", session_id, outcome);
                outcome
            }
            Ok(handle) => {
                let snapshot = handle.load_full();
                let result = self
                    .sink
                    .write(&snapshot, updates, removed)
                    .await
                    .map_err(|e| e.to_string());
                match &result {
                    Ok(()) => {
                        debug!("Persisted session {}", session_id);
                        self.failed.remove(&session_id);
                    }
                    Err(e) => {
                        warn!("Persist of session {} failed: {}", session_id, e);
                        // Retried by the next write of the session (it writes
                        // every row storage lacks) or by `flush_all`.
                        self.failed.insert(session_id, Arc::downgrade(&handle));
                    }
                }
                *slot.last_result.lock() = result.clone();
                result
            }
        };

        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
    }

    /// Run `f` while no write of `session_id` is in progress, so it is
    /// ordered with the persists queued before it.
    pub(crate) async fn exclusive<T, F>(&self, session_id: Uuid, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let slot = self.slots.get(&session_id).map(|s| Arc::clone(&s));
        let _write = match &slot {
            Some(slot) => Some(slot.write_lock.lock().await),
            None => None,
        };
        f.await
    }

    /// Delete a session: retire every handle registered for it so far (and
    /// `live`, if unregistered), wait for a running write, delete it from
    /// storage, then drop the persists queued for the deleted handles.
    pub(crate) async fn delete(
        self: &Arc<Self>,
        session_id: Uuid,
        live: Option<&SessionHandle>,
    ) -> anyhow::Result<()> {
        if let Some(handle) = live {
            self.tombstone(session_id, handle);
        }
        {
            let mut lineage = self.lineages.entry(session_id).or_default();
            lineage.prune();
            lineage.deleted_below = lineage.next;
        }
        self.failed.remove(&session_id);

        let slot = self.slots.get(&session_id).map(|s| Arc::clone(&s));
        let Some(slot) = slot else {
            return self.sink.delete(session_id).await;
        };

        let _write = slot.write_lock.lock().await;
        let result = self.sink.delete(session_id).await;
        // Unregister first: requests from here on start a new slot, and
        // whatever reached this one is judged by its source below.
        self.slots
            .remove_if(&session_id, |_, s| Arc::ptr_eq(s, &slot));
        let leftover = std::mem::take(&mut *slot.pending.lock());
        match &leftover.source {
            Some(handle) if !self.is_deleted(session_id, handle) => {
                // A request of a session re-created under the same id.
                self.queue(session_id, leftover);
            }
            _ => leftover.fail(SESSION_DELETED),
        }
        result
    }
}

/// Merge `work` into `pending`. A removal cancels a queued row of the same
/// update.
fn merge(pending: &parking_lot::Mutex<Pending>, work: Pending) {
    let mut p = pending.lock();
    if let Some(source) = work.source {
        p.source = Some(source);
    }
    p.updates.extend(work.updates);
    if !work.removed.is_empty() {
        p.updates.retain(|u| !work.removed.contains(&u.id));
        p.removed.extend(work.removed);
    }
    p.waiters.extend(work.waiters);
}

#[cfg(test)]
mod tests {
    use super::*;
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
            updates: Vec<ContextUpdate>,
            removed: Vec<Uuid>,
        ) -> anyhow::Result<()> {
            if self.running.fetch_add(1, Ordering::SeqCst) > 0 {
                self.overlapped.fetch_add(1, Ordering::SeqCst);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.running.fetch_sub(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("storage down");
            }
            self.writes
                .lock()
                .push((session.name(), updates.len(), removed.len()));
            Ok(())
        }

        async fn delete(&self, _session_id: Uuid) -> anyhow::Result<()> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn handle(id: Uuid) -> SessionHandle {
        Arc::new(ArcSwap::new(Arc::new(ActiveSession::new(
            id,
            Some("v0".into()),
            None,
        ))))
    }

    fn rename(handle: &SessionHandle, name: &str) {
        let mut next = (**handle.load()).clone();
        next.set_name(Some(name.to_string()));
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
    async fn flush(
        persister: &Arc<SessionPersister>,
        id: Uuid,
        h: &SessionHandle,
    ) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        persister.enqueue(id, h, vec![], vec![], Some(tx));
        rx.await.unwrap()
    }

    /// Let the writer task start its (slow) write.
    async fn let_writer_start() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    #[tokio::test]
    async fn queued_persists_coalesce_and_the_newest_version_lands_last() {
        let sink = Arc::new(RecordingSink::default());
        let persister = SessionPersister::new(sink.clone());
        let id = Uuid::new_v4();
        let h = handle(id);
        persister.register(id, &h);

        const K: usize = 20;
        rename(&h, "v1");
        persister.enqueue(id, &h, vec![update()], vec![], None);
        let_writer_start().await;
        for i in 2..=K {
            rename(&h, &format!("v{i}"));
            persister.enqueue(id, &h, vec![update()], vec![], None);
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
        let persister = SessionPersister::new(sink.clone());
        let id = Uuid::new_v4();
        let h = handle(id);

        persister.enqueue(id, &h, vec![update()], vec![], None);
        let_writer_start().await;
        let doomed = update();
        persister.enqueue(id, &h, vec![doomed.clone()], vec![], None);
        persister.enqueue(id, &h, vec![], vec![doomed.id], None);
        flush(&persister, id, &h).await.unwrap();

        let writes = sink.writes.lock().clone();
        assert_eq!(writes[1].1, 0, "the removed update's row is not written");
        assert_eq!(writes[1].2, 1, "its removal is passed down");
    }

    #[tokio::test]
    async fn delete_drops_queued_persists_and_blocks_late_ones() {
        let sink = Arc::new(RecordingSink::default());
        let persister = SessionPersister::new(sink.clone());
        let id = Uuid::new_v4();
        let h = handle(id);
        persister.register(id, &h);

        persister.enqueue(id, &h, vec![update()], vec![], None);
        let_writer_start().await;
        let (tx, queued) = oneshot::channel();
        persister.enqueue(id, &h, vec![update()], vec![], Some(tx));

        persister.delete(id, None).await.unwrap();
        assert_eq!(queued.await.unwrap(), Err(SESSION_DELETED.to_string()));
        assert!(persister.is_deleted(id, &h));

        // A task still holding the handle cannot write the session back.
        assert_eq!(
            flush(&persister, id, &h).await,
            Err(SESSION_DELETED.to_string())
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
        let persister = SessionPersister::new(sink.clone());
        let id = Uuid::new_v4();
        let old = handle(id);
        persister.register(id, &old);

        persister.enqueue(id, &old, vec![update()], vec![], None);
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
        persister.enqueue(id, &fresh, vec![update()], vec![], Some(tx));

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
        let persister = SessionPersister::new(sink.clone());
        let id = Uuid::new_v4();
        let h = handle(id);

        persister.delete(id, Some(&h)).await.unwrap();
        assert_eq!(
            flush(&persister, id, &h).await,
            Err(SESSION_DELETED.to_string())
        );
        assert!(sink.writes.lock().is_empty());
    }

    #[tokio::test]
    async fn superseded_handle_writes_the_newest_or_nothing() {
        let sink = Arc::new(RecordingSink::default());
        let persister = SessionPersister::new(sink.clone());
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
            Err(HANDLE_SUPERSEDED.to_string())
        );
        assert_eq!(sink.writes.lock().len(), 1);
    }

    #[tokio::test]
    async fn failed_writes_are_tracked_and_retried_by_flush_all() {
        let sink = Arc::new(RecordingSink::default());
        let persister = SessionPersister::new(sink.clone());
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
    async fn wait_idle_returns_after_queued_writes() {
        let sink = Arc::new(RecordingSink::default());
        let persister = SessionPersister::new(sink.clone());
        let id = Uuid::new_v4();
        let h = handle(id);

        persister.enqueue(id, &h, vec![update()], vec![], None);
        persister.wait_idle(id).await;
        assert_eq!(sink.writes.lock().len(), 1);
        persister.wait_idle(Uuid::new_v4()).await; // nothing queued: no wait
    }
}
