// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The session cache of a memory system and everything derived per cached
//! session. One path installs sessions ([`SessionManager::install`]) and it
//! is the one that evicts: sessions over the capacity leave the cache
//! (least recently used first, pinned ones stay; see
//! [`SessionCache`]) together with what was derived from them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use dashmap::DashMap;
use tracing::{debug, error};
use uuid::Uuid;

use post_cortex_core::session::active_session::StoredSession;

use crate::memory_system::session_cache::{CachedSession, SessionHandle};

use crate::memory_system::RuntimeError;
use crate::memory_system::derived_work::DerivedWork;
use crate::memory_system::persister::SessionPersister;
use crate::memory_system::session_cache::SessionCache;
use post_cortex_ports::SessionStore;

/// Retrieval graph per session, with the session's vector and relation
/// counts it was built from; rebuilt when either changes.
pub(crate) type SessionGraphs = DashMap<
    Uuid,
    (
        usize,
        usize,
        Arc<post_cortex_core::graph::retrieval_graph::RetrievalGraph<String>>,
    ),
>;

/// Session cache and per-session derived state.
pub struct SessionManager {
    /// Cached session handles
    pub sessions: SessionCache,

    /// The session port, for loads.
    sessions_store: Arc<dyn SessionStore>,

    /// The one writer path; loads wait for its queued writes.
    persister: Arc<SessionPersister>,

    /// Embeddings and graph merges in flight: their sessions are pinned.
    derived_work: Arc<DerivedWork>,

    /// One gate per session being loaded, so concurrent cache misses load
    /// it once and all get the same handle.
    loading: DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>,

    /// Record validity per cached session, for search results
    pub validity: crate::memory_system::neighbours::ValidityCache,

    /// Retrieval graph per cached session (see `graph_candidates`)
    pub(crate) retrieval_graphs: SessionGraphs,

    /// Total number of sessions created
    pub session_count: Arc<AtomicUsize>,
    /// Total number of session operations performed
    pub total_session_operations: Arc<AtomicU64>,
}

impl SessionManager {
    /// A session manager for `cache_capacity` sessions loaded from
    /// `sessions_store`; sessions with jobs in `derived_work` or with writes
    /// `persister` has not finished are not evicted.
    pub(crate) fn new(
        cache_capacity: usize,
        sessions_store: Arc<dyn SessionStore>,
        persister: Arc<SessionPersister>,
        derived_work: Arc<DerivedWork>,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            sessions: SessionCache::new(cache_capacity)?,
            sessions_store,
            persister,
            derived_work,
            loading: DashMap::new(),
            validity: Default::default(),
            retrieval_graphs: Default::default(),
            session_count: Arc::new(AtomicUsize::new(0)),
            total_session_operations: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Cached handle of `session_id`, marking it as used.
    pub fn cached(&self, session_id: Uuid) -> Option<SessionHandle> {
        self.sessions.get(&session_id)
    }

    /// Get a session from the cache or storage; `Ok(None)` when storage has
    /// no such session. Nothing here creates a session (only
    /// `MemoryRuntime::create_session` does), so a write to an
    /// unknown or deleted id can never bring it into existence. Any load
    /// failure (I/O, corrupt data, timeout) is an error, never an empty
    /// session.
    pub async fn get_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<SessionHandle>, RuntimeError> {
        self.total_session_operations
            .fetch_add(1, Ordering::Relaxed);
        if let Some(session_arc) = self.cached(session_id) {
            return Ok(Some(session_arc));
        }

        // Single flight: the first miss loads, concurrent misses wait for
        // it and then find the session cached. Without this each would
        // install its own handle and writes to the losing one would vanish.
        self.with_load_gate(session_id, async {
            match self.sessions.peek(&session_id) {
                Some(session_arc) => Ok(Some(session_arc)),
                None => self.load_uncached(session_id).await,
            }
        })
        .await
    }

    /// Run `f` holding the session's loading gate: no load of the session
    /// runs meanwhile. Deletes take it so a load cannot read the session
    /// before the delete and install it after.
    pub async fn with_load_gate<T>(
        &self,
        session_id: Uuid,
        f: impl std::future::Future<Output = T>,
    ) -> T {
        let gate = Arc::clone(
            &self
                .loading
                .entry(session_id)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        );
        let result = {
            let _guard = gate.lock().await;
            f.await
        };
        // Drop the gate once nobody else waits on it (map + this clone).
        self.loading.remove_if(&session_id, |_, g| {
            Arc::ptr_eq(g, &gate) && Arc::strong_count(g) <= 2
        });
        result
    }

    /// Load from storage and install in the cache; `Ok(None)` when storage
    /// has no such session. The caller holds the session's loading gate.
    async fn load_uncached(&self, session_id: Uuid) -> Result<Option<SessionHandle>, RuntimeError> {
        // Writes still queued from an evicted copy must land first, or the
        // load would read an older session than the one committed in memory.
        self.persister.wait_idle(session_id).await?;
        let Some(stored) = self.load_stored(session_id).await? else {
            return Ok(None);
        };

        Ok(Some(self.install(session_id, stored)))
    }

    /// The stored session, bypassing the cache. `Ok(None)` means storage
    /// holds no such session; any other failure is an error, never `None`,
    /// so a read failure is not mistaken for a missing session.
    pub(crate) async fn load_stored(
        &self,
        session_id: Uuid,
    ) -> Result<Option<StoredSession>, RuntimeError> {
        match self.sessions_store.load_session(session_id).await {
            Ok(session) => Ok(Some(session)),
            Err(e) if e.is_not_found() => Ok(None),
            Err(e) => {
                error!("Storage error loading session {session_id}: {e}");
                Err(e.into())
            }
        }
    }

    /// Put a session in the cache as its current handle (registered with
    /// the persister, so older handles of it are superseded), then evict
    /// what is over the capacity.
    pub fn install(&self, session_id: Uuid, stored: StoredSession) -> SessionHandle {
        let session_arc = Arc::new(CachedSession::new(stored.session, stored.vectorized));
        self.persister.register(session_id, &session_arc);
        self.sessions.insert(session_id, Arc::clone(&session_arc));
        self.evict_over_capacity();
        session_arc
    }

    /// Evict the least recently used sessions over the capacity that are
    /// not pinned (in use, with writes not yet stored, or with derived work
    /// queued), with their derived state. Returns the evicted ids.
    pub fn evict_over_capacity(&self) -> Vec<Uuid> {
        let evicted = self.sessions.evict_over_capacity(|id| {
            self.persister.has_unflushed(id) || self.derived_work.is_busy(id)
        });
        if !evicted.is_empty() {
            debug!("Evicted {} sessions from the cache", evicted.len());
            self.drop_derived_state();
        }
        evicted
    }

    /// Forget a deleted session: its cache entry and derived state.
    pub fn forget(&self, session_id: &Uuid) -> Option<SessionHandle> {
        let handle = self.sessions.remove(session_id);
        self.drop_derived_state();
        handle
    }

    /// Drop what was derived from `session_id` (its records changed in a
    /// way the caches cannot see, e.g. an import or a purge).
    pub fn invalidate_derived(&self, session_id: &Uuid) {
        {
            self.validity.retain(|id| id != session_id);
            self.retrieval_graphs.remove(session_id);
        }
    }

    /// Drop the derived state of every session that is not cached.
    fn drop_derived_state(&self) {
        {
            self.validity.retain(|id| self.sessions.contains(id));
            self.retrieval_graphs
                .retain(|id, _| self.sessions.contains(id));
        }
    }
}
