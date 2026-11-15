// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Which handle of a session may write: handles are registered per session
//! with a generation; a superseded handle writes through the newest live one,
//! a deleted incarnation's handles are tombstoned and write nothing.

use std::sync::{Arc, Weak};

use dashmap::DashMap;
use uuid::Uuid;

use crate::memory_system::session_cache::{CachedSession, SessionHandle};

/// How a persist through a given handle is treated.
pub enum Standing {
    /// The handle is the session's current one (or was never registered).
    Current,
    /// A newer handle replaced this one; write through it instead, or drop
    /// the request when it is gone too.
    Superseded(Option<SessionHandle>),
    /// The session was deleted after this handle was created.
    Deleted,
}

/// Handles registered for one session id.
#[derive(Default)]
struct Lineage {
    /// Live registered handles with their generation.
    members: Vec<(Weak<CachedSession>, u64)>,
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

/// The handles of every session: who is current, who was superseded, who
/// belongs to a deleted incarnation.
#[derive(Default)]
pub struct Lineages {
    lineages: DashMap<Uuid, Lineage>,
    /// Unregistered handles of deleted sessions that something may still hold.
    tombstones: DashMap<Uuid, Vec<Weak<CachedSession>>>,
}

impl Lineages {
    /// Register a freshly loaded or created handle as the session's current
    /// one; older registered handles become superseded.
    pub fn register(&self, session_id: Uuid, handle: &SessionHandle) {
        let mut lineage = self.lineages.entry(session_id).or_default();
        lineage.prune();
        let generation = lineage.next;
        lineage.next += 1;
        lineage.members.push((Arc::downgrade(handle), generation));
    }

    /// Mark every handle registered so far (and `live`, registered or not)
    /// as belonging to a deleted incarnation of the session.
    pub fn retire(&self, session_id: Uuid, live: Option<&SessionHandle>) {
        if let Some(handle) = live {
            self.tombstone(session_id, handle);
        }
        let mut lineage = self.lineages.entry(session_id).or_default();
        lineage.prune();
        lineage.deleted_below = lineage.next;
    }

    /// The handle to retry a failed write of `session_id` from: the one it
    /// failed with, or the session's newest live one.
    pub fn retry_handle(
        &self,
        session_id: Uuid,
        failed: &Weak<CachedSession>,
    ) -> Option<SessionHandle> {
        failed.upgrade().or_else(|| {
            self.lineages
                .get(&session_id)
                .and_then(|l| l.newest_alive())
                .map(|(h, _)| h)
        })
    }

    /// How a persist through `handle` is treated.
    pub fn standing(&self, session_id: Uuid, handle: &SessionHandle) -> Standing {
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
    pub fn is_deleted(&self, session_id: Uuid, handle: &SessionHandle) -> bool {
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
}
