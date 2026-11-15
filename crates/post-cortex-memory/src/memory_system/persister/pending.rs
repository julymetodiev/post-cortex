// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The queue of one session: pending update rows, removals and waiters,
//! merged while a write is in flight, and what a failed write carries over.

use std::sync::Weak;

use post_cortex_ports::SessionChanges;
use tokio::sync::oneshot;
use tracing::debug;
use uuid::Uuid;

use crate::memory_system::persister::PersistError;
use crate::memory_system::session_cache::{CachedSession, SessionHandle};

/// The outcome of the write that covers a request.
pub type Outcome = Result<(), PersistError>;

/// Told the outcome of the write that covers its request.
pub type Waiter = oneshot::Sender<Outcome>;

/// Tell `waiter` the outcome. A waiter that stopped waiting has nothing to
/// be told: the write's own result is logged and retried by the persister.
pub fn notify(session_id: Uuid, waiter: Waiter, outcome: Outcome) {
    if waiter.send(outcome).is_err() {
        debug!("A waiter of session {session_id} stopped waiting before its write finished");
    }
}

/// Work queued for one session.
#[derive(Default)]
pub struct Pending {
    /// Session to snapshot at write time; `None` when only waiters queued.
    pub source: Option<SessionHandle>,
    /// What the next write changes: record rows, removals, whether the
    /// whole graph changed (the vectorized ids join at write time).
    pub changes: SessionChanges,
    /// Notified once a write covering their request finished.
    pub waiters: Vec<Waiter>,
}

impl Pending {
    /// Nothing to write and nobody to tell.
    pub fn is_empty(&self) -> bool {
        self.source.is_none() && self.changes.is_empty() && self.waiters.is_empty()
    }

    /// Tell every waiter that its request will not be written.
    pub fn fail(self, session_id: Uuid, error: &PersistError) {
        for waiter in self.waiters {
            notify(session_id, waiter, Err(error.clone()));
        }
    }

    /// Merge `work` into this queue (see [`SessionChanges::merge`]: a later
    /// row or removal of a record wins).
    pub fn merge(&mut self, work: Pending) {
        if let Some(source) = work.source {
            self.source = Some(source);
        }
        self.changes.merge(work.changes);
        self.waiters.extend(work.waiters);
    }
}

/// One session's queue and write lock.
pub struct Slot {
    /// Held for the duration of every storage write of this session, so
    /// writes (and deletes) of one session never overlap.
    pub write_lock: tokio::sync::Mutex<()>,
    /// Work queued since the running write started.
    pub pending: parking_lot::Mutex<Pending>,
    /// Outcome of the last write, reported to waiters that queued while
    /// that write was already running with nothing new to add.
    pub last_result: parking_lot::Mutex<Outcome>,
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

/// A session whose last write failed: the handle to retry from and the
/// changes that write carried, written again by the next one.
pub struct Failed {
    /// The handle the write failed with.
    pub handle: Weak<CachedSession>,
    /// The changes the failed write carried.
    pub changes: SessionChanges,
}

impl Failed {
    /// The changes of this failed write followed by those of the write
    /// retrying it, whose own versions win.
    pub fn carry_over(self, newer: SessionChanges) -> SessionChanges {
        let mut changes = self.changes;
        changes.merge(newer);
        changes
    }
}
