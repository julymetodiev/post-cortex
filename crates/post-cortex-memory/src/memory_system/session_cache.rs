// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The cached session handles of a memory system. Least recently used
//! sessions are evicted once the cache holds more than its capacity, but
//! never a pinned one: a session still in use (anyone else holds its
//! handle), with writes not yet stored, or with derived work queued.
//! Eviction checks and removes under the map's lock, so nobody can take the
//! handle in between.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

use crate::memory_system::vectorized::VectorizedIds;

/// A cached session: its current version (each change publishes a new
/// one) and the ids of its embedded records, which every version shares.
/// Derefs to the current version's `ArcSwap`.
#[derive(Debug)]
pub struct CachedSession {
    current: ArcSwap<ActiveSession>,
    vectorized: VectorizedIds,
}

impl CachedSession {
    /// `session` with the embedded record ids `vectorized` (as stored).
    pub fn new(session: ActiveSession, vectorized: impl IntoIterator<Item = Uuid>) -> Self {
        Self {
            current: ArcSwap::new(Arc::new(session)),
            vectorized: VectorizedIds::new(vectorized),
        }
    }

    /// Which records of the session have their embedding in the index.
    pub fn vectorized(&self) -> &VectorizedIds {
        &self.vectorized
    }
}

impl std::ops::Deref for CachedSession {
    type Target = ArcSwap<ActiveSession>;

    fn deref(&self) -> &Self::Target {
        &self.current
    }
}

/// Live, shared handle to a cached session.
pub type SessionHandle = Arc<CachedSession>;

/// One cached session.
struct Entry {
    handle: SessionHandle,
    /// Logical time of the last access (see [`SessionCache::clock`]).
    last_access: AtomicU64,
}

/// Counters of the cache.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionCacheStats {
    /// Sessions cached now
    pub size: usize,
    /// Sessions held at most before eviction starts
    pub capacity: usize,
    /// Lookups that found the session
    pub hits: u64,
    /// Lookups that did not
    pub misses: u64,
    /// Sessions evicted
    pub evictions: u64,
}

impl SessionCacheStats {
    /// Share of lookups that found the session (0 when there were none).
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Session handles by id; see the module docs.
pub struct SessionCache {
    entries: DashMap<Uuid, Entry>,
    capacity: usize,
    /// Logical clock of accesses: cheaper than reading the time, and ties
    /// cannot happen.
    clock: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

impl SessionCache {
    /// A cache for `capacity` sessions (at least one).
    ///
    /// # Errors
    /// `capacity` is 0.
    pub fn new(capacity: usize) -> Result<Self, crate::memory_system::RuntimeError> {
        if capacity == 0 {
            return Err(crate::memory_system::RuntimeError::InvalidConfig(
                "session cache capacity must be at least 1",
            ));
        }
        Ok(Self {
            entries: DashMap::new(),
            capacity,
            clock: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        })
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The cached handle of `id`, marking it as used.
    pub fn get(&self, id: &Uuid) -> Option<SessionHandle> {
        match self.entries.get(id) {
            Some(entry) => {
                entry.last_access.store(self.tick(), Ordering::Relaxed);
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(Arc::clone(&entry.handle))
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// The cached handle of `id`, without counting a lookup or marking it.
    pub fn peek(&self, id: &Uuid) -> Option<SessionHandle> {
        self.entries.get(id).map(|e| Arc::clone(&e.handle))
    }

    /// Whether `id` is cached.
    pub fn contains(&self, id: &Uuid) -> bool {
        self.entries.contains_key(id)
    }

    /// Cache `handle` as the session's (replacing an older one). Does not
    /// evict; see [`evict_over_capacity`](Self::evict_over_capacity).
    pub fn insert(&self, id: Uuid, handle: SessionHandle) {
        self.entries.insert(
            id,
            Entry {
                handle,
                last_access: AtomicU64::new(self.tick()),
            },
        );
    }

    /// Drop `id` from the cache (the session was deleted); returns its
    /// handle.
    pub fn remove(&self, id: &Uuid) -> Option<SessionHandle> {
        self.entries.remove(id).map(|(_, e)| e.handle)
    }

    /// Sessions cached now.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no session is cached.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Evict least recently used sessions until at most `capacity` remain,
    /// skipping the pinned ones: sessions whose handle someone else holds
    /// and those `pinned` names (writes not stored, derived work queued).
    /// Returns the evicted ids.
    pub fn evict_over_capacity(&self, pinned: impl Fn(Uuid) -> bool) -> Vec<Uuid> {
        let excess = self.entries.len().saturating_sub(self.capacity);
        if excess == 0 {
            return Vec::new();
        }
        let mut by_age: Vec<(u64, Uuid)> = self
            .entries
            .iter()
            .map(|e| (e.last_access.load(Ordering::Relaxed), *e.key()))
            .collect();
        by_age.sort_unstable();
        let mut evicted = Vec::with_capacity(excess);
        for (_, id) in by_age {
            if evicted.len() == excess {
                break;
            }
            let removed = self.entries.remove_if(&id, |id, entry| {
                Arc::strong_count(&entry.handle) == 1 && !pinned(*id)
            });
            if removed.is_some() {
                evicted.push(id);
            }
        }
        self.evictions
            .fetch_add(evicted.len() as u64, Ordering::Relaxed);
        evicted
    }

    /// The counters.
    pub fn stats(&self) -> SessionCacheStats {
        SessionCacheStats {
            size: self.entries.len(),
            capacity: self.capacity,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}
