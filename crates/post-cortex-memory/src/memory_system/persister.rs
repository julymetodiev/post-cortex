//! Coalescing, per-session ordered session persistence.
//!
//! Every write path used to send a full session snapshot to the storage
//! actor, which spawned one task per snapshot. Nothing coalesced them (K
//! updates meant K full blob writes) and task start order is not send
//! order, so an older snapshot could land after a newer one.
//!
//! [`SessionPersister`] keeps one slot per session instead. A persist
//! request names the session's live `ArcSwap` and the update rows to write;
//! requests that queue up while a write is in flight merge into the slot.
//! One writer task per session drains it, loading the newest snapshot from
//! the `ArcSwap` at write time, so K queued requests cost one blob write
//! (carrying all K update rows) and the last write always holds the latest
//! state.
//!
//! Deleting a session drains its slot under the same per-session write
//! lock and tombstones the deleted `ArcSwap`, so a persist still in flight
//! for it (a pipeline worker, a vectorization task) cannot write the
//! session back.

use std::sync::{Arc, Weak};

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

/// Where [`SessionPersister`] writes. Implemented for the storage trait
/// object; a seam so ordering and coalescing can be tested.
#[async_trait]
pub(crate) trait PersistSink: Send + Sync {
    /// Write the session blob plus `updates` (and any update rows the
    /// backend has not stored yet).
    async fn write(
        &self,
        session: &ActiveSession,
        updates: Vec<ContextUpdate>,
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
    ) -> anyhow::Result<()> {
        self.0
            .save_session_with_updates(session, session.id(), updates)
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
    /// Notified once a write covering their request finished.
    waiters: Vec<Waiter>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.source.is_none() && self.updates.is_empty() && self.waiters.is_empty()
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

/// Coalescing per-session writer. See the module docs.
pub(crate) struct SessionPersister {
    sink: Arc<dyn PersistSink>,
    slots: DashMap<Uuid, Arc<Slot>>,
    /// Session handles deleted while something may still hold them.
    tombstones: DashMap<Uuid, Vec<Weak<ArcSwap<ActiveSession>>>>,
}

impl SessionPersister {
    pub(crate) fn new(sink: Arc<dyn PersistSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            slots: DashMap::new(),
            tombstones: DashMap::new(),
        })
    }

    /// Whether `handle` belongs to a session that has been deleted.
    pub(crate) fn is_deleted(&self, session_id: Uuid, handle: &SessionHandle) -> bool {
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

    /// Queue a write of `handle`'s latest version plus `updates`. With a
    /// `waiter`, it is told the outcome of the write that covers this call.
    pub(crate) fn enqueue(
        self: &Arc<Self>,
        session_id: Uuid,
        handle: &SessionHandle,
        updates: Vec<ContextUpdate>,
        waiter: Option<Waiter>,
    ) {
        if self.is_deleted(session_id, handle) {
            debug!("Dropping persist of deleted session {}", session_id);
            if let Some(w) = waiter {
                let _ = w.send(Err(SESSION_DELETED.to_string()));
            }
            return;
        }

        // The pending queue is extended while the map entry is held, and the
        // writer only unregisters an empty slot under that same entry lock,
        // so a request can never land in a slot nobody drains.
        let spawn = match self.slots.entry(session_id) {
            Entry::Occupied(slot) => {
                push(&slot.get().pending, handle, updates, waiter);
                None
            }
            Entry::Vacant(vacant) => {
                let slot = Arc::new(Slot::default());
                push(&slot.pending, handle, updates, waiter);
                vacant.insert(Arc::clone(&slot));
                Some(slot)
            }
        };

        if let Some(slot) = spawn {
            let this = Arc::clone(self);
            tokio::spawn(async move { this.drain(session_id, slot).await });
        }
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
            waiters,
        } = batch;

        let result = match source {
            // Only waiters: the write they wait for already ran.
            None => slot.last_result.lock().clone(),
            Some(handle) if !registered || self.is_deleted(session_id, &handle) => {
                debug!("Skipping persist of deleted session {}", session_id);
                Err(SESSION_DELETED.to_string())
            }
            Some(handle) => {
                let snapshot = handle.load_full();
                let result = self
                    .sink
                    .write(&snapshot, updates)
                    .await
                    .map_err(|e| e.to_string());
                match &result {
                    Ok(()) => debug!("Persisted session {}", session_id),
                    Err(e) => warn!("Persist of session {} failed: {}", session_id, e),
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

    /// Delete a session: drop its queued persists, tombstone `live` and the
    /// queued handle so in-flight persists cannot resurrect it, and delete
    /// it from storage once any running write finished.
    pub(crate) async fn delete(
        &self,
        session_id: Uuid,
        live: Option<&SessionHandle>,
    ) -> anyhow::Result<()> {
        if let Some(handle) = live {
            self.tombstone(session_id, handle);
        }

        let slot = self.slots.get(&session_id).map(|s| Arc::clone(&s));
        let Some(slot) = slot else {
            return self.sink.delete(session_id).await;
        };

        let _write = slot.write_lock.lock().await;
        let dropped = std::mem::take(&mut *slot.pending.lock());
        if let Some(handle) = &dropped.source {
            self.tombstone(session_id, handle);
        }
        for waiter in dropped.waiters {
            let _ = waiter.send(Err(SESSION_DELETED.to_string()));
        }
        self.slots
            .remove_if(&session_id, |_, s| Arc::ptr_eq(s, &slot));
        self.sink.delete(session_id).await
    }
}

fn push(
    pending: &parking_lot::Mutex<Pending>,
    handle: &SessionHandle,
    updates: Vec<ContextUpdate>,
    waiter: Option<Waiter>,
) {
    let mut p = pending.lock();
    p.source = Some(Arc::clone(handle));
    p.updates.extend(updates);
    p.waiters.extend(waiter);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Sink that records every write and takes a while doing it, so
    /// requests pile up behind a running write.
    #[derive(Default)]
    struct RecordingSink {
        /// (session name, update rows) per blob write, in write order.
        writes: parking_lot::Mutex<Vec<(Option<String>, usize)>>,
        deletes: AtomicUsize,
        running: AtomicUsize,
        overlapped: AtomicUsize,
    }

    #[async_trait]
    impl PersistSink for RecordingSink {
        async fn write(
            &self,
            session: &ActiveSession,
            updates: Vec<ContextUpdate>,
        ) -> anyhow::Result<()> {
            if self.running.fetch_add(1, Ordering::SeqCst) > 0 {
                self.overlapped.fetch_add(1, Ordering::SeqCst);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.writes.lock().push((session.name(), updates.len()));
            self.running.fetch_sub(1, Ordering::SeqCst);
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
        }
    }

    /// Persist `h` and wait for the write.
    async fn flush(
        persister: &Arc<SessionPersister>,
        id: Uuid,
        h: &SessionHandle,
    ) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        persister.enqueue(id, h, vec![], Some(tx));
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

        const K: usize = 20;
        rename(&h, "v1");
        persister.enqueue(id, &h, vec![update()], None);
        let_writer_start().await;
        for i in 2..=K {
            rename(&h, &format!("v{i}"));
            persister.enqueue(id, &h, vec![update()], None);
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
    async fn delete_drops_queued_persists_and_blocks_late_ones() {
        let sink = Arc::new(RecordingSink::default());
        let persister = SessionPersister::new(sink.clone());
        let id = Uuid::new_v4();
        let h = handle(id);

        persister.enqueue(id, &h, vec![update()], None);
        let_writer_start().await;
        let (tx, queued) = oneshot::channel();
        persister.enqueue(id, &h, vec![update()], Some(tx));

        persister.delete(id, Some(&h)).await.unwrap();
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

        // A new session under the same id is not affected.
        let fresh = handle(id);
        assert!(!persister.is_deleted(id, &fresh));
        flush(&persister, id, &fresh).await.unwrap();
        assert_eq!(sink.writes.lock().len(), 2);
    }
}
