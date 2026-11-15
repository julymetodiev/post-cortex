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
//! Handles are registered per session with a generation number (see
//! [`Lineages`]). A handle that a newer one superseded (the session was
//! evicted and loaded again) never writes its own, older snapshot: its
//! requests go to the newest live handle or are dropped. Deleting a session
//! retires every handle registered so far and drains the slot under the
//! per-session write lock, so a persist still in flight for it (a pipeline
//! worker, a vectorization task) cannot write the session back.
//!
//! Each request says what it changes ([`SessionChanges`]): record rows,
//! removals, whether the whole graph changed; the write adds the vectorized
//! ids marked and unmarked since the last write. Storage writes those rows
//! only, so a write costs what it changes, not the session's size.
//!
//! A failed write keeps its changes: the session's next write (any persist
//! of it, `wait_idle` or `flush_all`) carries them again, so a retirement
//! (`forget`) or a removal of the failed write still reaches storage. A storage adapter that panics fails the write instead of the
//! writer task, so waiters always get an answer and the slot keeps draining.
//! A write that succeeds announces the record changes it carried on the
//! change log (see [`crate::memory_system::change_log`]), those of earlier failed
//! writes first.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Weak};
use std::time::Duration;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use futures::FutureExt;
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_ports::{PortError, SessionChanges};
use tokio::sync::oneshot;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::memory_system::change_log::{Change, ChangeLog};
use crate::memory_system::persister::lineage::{Lineages, Standing};
use crate::memory_system::persister::pending::{Failed, Outcome, Pending, Slot, Waiter, notify};
use crate::memory_system::record_events::events_of_write;
use crate::memory_system::session_cache::{CachedSession, SessionHandle};

mod lineage;
mod pending;
mod sink;

pub use sink::{PersistError, PersistSink, StorageSink};

/// Coalescing per-session writer. See the module docs.
pub struct SessionPersister {
    sink: Arc<dyn PersistSink>,
    slots: DashMap<Uuid, Arc<Slot>>,
    lineages: Lineages,
    /// Sessions whose last write failed, with what to retry.
    failed: DashMap<Uuid, Failed>,
    /// Where the record changes of finished writes are published.
    changes: ChangeLog,
}

impl SessionPersister {
    /// A persister writing through `sink`, publishing the record changes of
    /// its finished writes to `changes`.
    pub fn new(sink: Arc<dyn PersistSink>, changes: ChangeLog) -> Arc<Self> {
        Arc::new(Self {
            sink,
            slots: DashMap::new(),
            lineages: Lineages::default(),
            failed: DashMap::new(),
            changes,
        })
    }

    /// Register a freshly loaded or created handle as the session's current
    /// one; older registered handles become superseded.
    pub fn register(&self, session_id: Uuid, handle: &SessionHandle) {
        self.lineages.register(session_id, handle);
    }

    /// Whether `handle` belongs to a session that has been deleted.
    pub fn is_deleted(&self, session_id: Uuid, handle: &SessionHandle) -> bool {
        self.lineages.is_deleted(session_id, handle)
    }

    /// Whether the session has writes queued or running, or its last write
    /// failed: evicting it now could lose committed data.
    pub fn has_unflushed(&self, session_id: Uuid) -> bool {
        self.slots.contains_key(&session_id) || self.failed.contains_key(&session_id)
    }

    /// How many sessions' last write failed (each waits for its retry).
    pub fn failing(&self) -> usize {
        self.failed.len()
    }

    /// Queue a write of `handle`'s latest version with `changes`. With a
    /// `waiter`, it is told the outcome of the write that covers this call.
    fn enqueue(
        self: &Arc<Self>,
        session_id: Uuid,
        handle: &SessionHandle,
        changes: SessionChanges,
        waiter: Option<Waiter>,
    ) {
        let source = match self.lineages.standing(session_id, handle) {
            Standing::Current => Arc::clone(handle),
            Standing::Superseded(Some(newest)) => newest,
            Standing::Superseded(None) => {
                warn!("Dropping persist of session {session_id} through a superseded handle");
                if let Some(w) = waiter {
                    notify(session_id, w, Err(PersistError::HandleSuperseded));
                }
                return;
            }
            Standing::Deleted => {
                debug!("Dropping persist of deleted session {session_id}");
                if let Some(w) = waiter {
                    notify(session_id, w, Err(PersistError::SessionDeleted));
                }
                return;
            }
        };
        self.queue(
            session_id,
            Pending {
                source: Some(source),
                changes,
                waiters: waiter.into_iter().collect(),
            },
        );
    }

    /// Queue a background write of `session`'s latest version plus `updates`
    /// and return at once. Writes of one session are ordered and coalesced:
    /// requests queued while a write runs are merged into one blob write
    /// that snapshots `session` when it starts, so the stored blob always
    /// ends at the newest version. Failures are logged and retried.
    pub fn persist_nowait(self: &Arc<Self>, session: &SessionHandle, updates: Vec<ContextUpdate>) {
        let session_id = session.load().id();
        self.enqueue(session_id, session, SessionChanges::records(updates), None);
    }

    /// Like [`Self::persist_nowait`] for a change of the graph other than
    /// through records (an entity deleted or merged, the graph rebuilt):
    /// the write stores the session's whole graph.
    pub fn persist_graph_nowait(self: &Arc<Self>, session: &SessionHandle) {
        let session_id = session.load().id();
        self.enqueue(session_id, session, SessionChanges::graph(), None);
    }

    /// Records `removed` and the graph rebuilt without them (a purge, a
    /// file invalidation): their rows are deleted and the whole graph is
    /// written.
    pub fn persist_rebuild_nowait(self: &Arc<Self>, session: &SessionHandle, removed: Vec<Uuid>) {
        let session_id = session.load().id();
        self.enqueue(session_id, session, SessionChanges::rebuild(removed), None);
    }

    /// Like [`Self::persist_nowait`], and delete the rows of the `removed`
    /// updates. Saves never delete an update row otherwise, so every path
    /// that removes updates from a session must name them here.
    pub fn persist_removals_nowait(self: &Arc<Self>, session: &SessionHandle, removed: Vec<Uuid>) {
        let session_id = session.load().id();
        self.enqueue(session_id, session, SessionChanges::removals(removed), None);
    }

    /// Like [`Self::persist_nowait`], but wait until a write that covers
    /// this request (blob and `updates`' rows) has finished, and report its
    /// outcome. No deadline here: a caller that stops waiting cannot stop
    /// the write, so a timeout would report failure for a write that lands.
    ///
    /// # Errors
    /// The write failed, or the request was dropped (see [`PersistError`]).
    pub async fn persist_and_wait(
        self: &Arc<Self>,
        session: &SessionHandle,
        updates: Vec<ContextUpdate>,
    ) -> Outcome {
        let session_id = session.load().id();
        let (tx, rx) = oneshot::channel();
        self.enqueue(
            session_id,
            session,
            SessionChanges::records(updates),
            Some(tx),
        );
        rx.await.unwrap_or(Err(PersistError::WriterGone))
    }

    /// Merge `work` into the session's slot, starting its writer if needed.
    fn queue(self: &Arc<Self>, session_id: Uuid, work: Pending) {
        // The pending queue is extended while the map entry is held, and the
        // writer only unregisters an empty slot under that same entry lock,
        // so a request can never land in a slot nobody drains.
        let spawn = match self.slots.entry(session_id) {
            Entry::Occupied(slot) => {
                slot.get().pending.lock().merge(work);
                None
            }
            Entry::Vacant(vacant) => {
                let slot = Arc::new(Slot::default());
                slot.pending.lock().merge(work);
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
    fn wait_queued(&self, session_id: Uuid) -> Option<oneshot::Receiver<Outcome>> {
        let slot = self.slots.get(&session_id)?;
        let (tx, rx) = oneshot::channel();
        slot.pending.lock().waiters.push(tx);
        Some(rx)
    }

    /// Wait until the session's queued writes are done, retrying a failed
    /// last write once, so a load that follows reads them.
    ///
    /// # Errors
    /// A queued write, or the retry of a failed one, did not land: what
    /// storage holds may be older than what was committed in memory.
    pub async fn wait_idle(self: &Arc<Self>, session_id: Uuid) -> Outcome {
        if let Some(rx) = self.wait_queued(session_id) {
            rx.await.unwrap_or(Err(PersistError::WriterGone))?;
        }
        let failed = self
            .failed
            .get(&session_id)
            .map(|f| f.handle.clone())
            .and_then(|w| self.lineages.retry_handle(session_id, &w));
        if let Some(handle) = failed {
            let (tx, rx) = oneshot::channel();
            self.enqueue(session_id, &handle, SessionChanges::default(), Some(tx));
            rx.await.unwrap_or(Err(PersistError::WriterGone))?;
        }
        Ok(())
    }

    /// Write everything queued and retry sessions whose last write failed;
    /// returns once every write finished or `limit` passed. Returns how
    /// many sessions are still not written. For graceful shutdown.
    pub async fn flush_all(self: &Arc<Self>, limit: Duration) -> usize {
        let mut receivers = Vec::new();
        let failed: Vec<(Uuid, Weak<CachedSession>)> = self
            .failed
            .iter()
            .map(|e| (*e.key(), e.value().handle.clone()))
            .collect();
        for (session_id, handle) in failed {
            if let Some(handle) = self.lineages.retry_handle(session_id, &handle) {
                let (tx, rx) = oneshot::channel();
                self.enqueue(session_id, &handle, SessionChanges::default(), Some(tx));
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
        let unwritten: HashSet<Uuid> = match tokio::time::timeout(limit, all).await {
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

    /// The handle a batch writes from, or the outcome its waiters get
    /// without a write.
    fn batch_source(
        &self,
        session_id: Uuid,
        slot: &Slot,
        source: Option<SessionHandle>,
        registered: bool,
    ) -> Result<SessionHandle, Outcome> {
        if !registered {
            // The slot was retired by a delete.
            return Err(Err(PersistError::SessionDeleted));
        }
        let Some(handle) = source else {
            // Only waiters: the write they wait for already ran.
            return Err(slot.last_result.lock().clone());
        };
        match self.lineages.standing(session_id, &handle) {
            Standing::Current => Ok(handle),
            Standing::Superseded(Some(newest)) => Ok(newest),
            Standing::Superseded(None) => Err(Err(PersistError::HandleSuperseded)),
            Standing::Deleted => Err(Err(PersistError::SessionDeleted)),
        }
    }

    async fn write_batch(&self, session_id: Uuid, slot: &Slot, batch: Pending, registered: bool) {
        let Pending {
            source,
            changes,
            waiters,
        } = batch;

        let result = match self.batch_source(session_id, slot, source, registered) {
            Err(outcome) => {
                debug!("Persist of session {session_id} skipped: {outcome:?}");
                outcome
            }
            Ok(handle) => {
                let result = self.write_from(session_id, &handle, changes).await;
                *slot.last_result.lock() = result.clone();
                result
            }
        };

        for waiter in waiters {
            notify(session_id, waiter, result.clone());
        }
    }

    /// Write `handle`'s latest snapshot with `changes` (after those of an
    /// earlier failed write) and the vectorized ids marked and unmarked
    /// since the last write, recording a failure for retry.
    async fn write_from(
        &self,
        session_id: Uuid,
        handle: &SessionHandle,
        changes: SessionChanges,
    ) -> Outcome {
        // Changes of an earlier failed write go first, so this write's own
        // versions of the same rows win.
        let mut changes = match self.failed.remove(&session_id) {
            Some((_, failed)) => failed.carry_over(changes),
            None => changes,
        };
        let (vectorized, unvectorized) = handle.vectorized().take_unsaved();
        changes.merge(SessionChanges {
            vectorized,
            unvectorized,
            ..Default::default()
        });
        let snapshot = handle.load_full();
        let events = if self.changes.has_subscribers() {
            events_of_write(session_id, &changes.updates, &changes.removed)
        } else {
            Vec::new()
        };
        let write = self.sink.write(&snapshot, changes.clone());
        let result = match AssertUnwindSafe(write).catch_unwind().await {
            Ok(written) => written.map_err(PersistError::Write),
            Err(_) => Err(PersistError::Panicked),
        };
        match &result {
            Ok(()) => {
                debug!("Persisted session {session_id}");
                for event in events {
                    self.changes.publish(Change::Record(event));
                }
            }
            Err(e) => {
                warn!("Persist of session {session_id} failed: {e}");
                // Retried by the next write of the session or by
                // `wait_idle` / `flush_all`, with these rows.
                self.failed.insert(
                    session_id,
                    Failed {
                        handle: Arc::downgrade(handle),
                        changes,
                    },
                );
            }
        }
        result
    }

    /// Run `f` while no write of `session_id` is in progress, so it is
    /// ordered with the persists queued before it.
    pub async fn exclusive<T, F>(&self, session_id: Uuid, f: F) -> T
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
    /// Returns whether storage had the session.
    ///
    /// # Errors
    /// Storage failed to delete the session.
    pub async fn delete(
        self: &Arc<Self>,
        session_id: Uuid,
        live: Option<&SessionHandle>,
    ) -> Result<bool, PortError> {
        self.lineages.retire(session_id, live);
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
            Some(handle) if !self.lineages.is_deleted(session_id, handle) => {
                // A request of a session re-created under the same id.
                self.queue(session_id, leftover);
            }
            _ => leftover.fail(session_id, &PersistError::SessionDeleted),
        }
        result
    }
}

#[cfg(test)]
mod tests;
