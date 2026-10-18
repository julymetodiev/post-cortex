use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use tracing::{error, info};
use uuid::Uuid;

use post_cortex_core::core::cache::SessionCache;
use post_cortex_core::session::active_session::ActiveSession;

use super::metrics::SessionManagerMetrics;
use super::storage_actor::StorageActorHandle;

/// Session manager using `DashMap` and atomic operations
pub struct SessionManager {
    /// Session cache
    pub sessions: SessionCache<Uuid, Arc<ArcSwap<ActiveSession>>>,

    /// Storage communication
    storage_actor: StorageActorHandle,

    /// One gate per session being loaded, so concurrent cache misses load
    /// it once and all get the same handle.
    loading: DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>,

    /// Total number of sessions created
    pub session_count: Arc<AtomicUsize>,
    /// Total number of session operations performed
    pub total_session_operations: Arc<AtomicU64>,
    /// Number of session cache hits
    pub session_cache_hits: Arc<AtomicU64>,
    /// Number of session cache misses
    pub session_cache_misses: Arc<AtomicU64>,
    /// Active sessions with their last-access timestamps
    pub active_sessions: DashMap<Uuid, Arc<AtomicU64>>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl SessionManager {
    /// Construct a new session manager wired to the given storage actor.
    pub fn new(cache_capacity: usize, storage_actor: StorageActorHandle) -> Result<Self, String> {
        let sessions = SessionCache::new(cache_capacity, "session_cache".to_string())
            .map_err(|e| format!("Failed to create session cache: {e}"))?;
        Ok(Self {
            sessions,
            storage_actor,
            loading: DashMap::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            total_session_operations: Arc::new(AtomicU64::new(0)),
            session_cache_hits: Arc::new(AtomicU64::new(0)),
            session_cache_misses: Arc::new(AtomicU64::new(0)),
            active_sessions: DashMap::new(),
        })
    }

    /// Cached handle of `session_id`, refreshing its access time.
    pub fn cached(&self, session_id: Uuid) -> Option<Arc<ArcSwap<ActiveSession>>> {
        let session_arc = self.sessions.get(&session_id)?;
        if let Some(access_time) = self.active_sessions.get(&session_id) {
            access_time.store(now_secs(), Ordering::Relaxed);
        }
        Some(session_arc)
    }

    /// Get or create session via actor communication.
    ///
    /// A new, empty session is created only when storage reports that the
    /// session does not exist. Any other load failure (I/O, corrupt data,
    /// timeout) is returned as an error, so a transient problem can never
    /// replace a stored session with an empty one.
    pub async fn get_or_create_session(
        &self,
        session_id: Uuid,
    ) -> Result<Arc<ArcSwap<ActiveSession>>, String> {
        self.load_or_create(session_id, true)
            .await?
            .ok_or_else(|| format!("Session {session_id} not found"))
    }

    /// Get a session from the cache or storage; `Ok(None)` when storage has
    /// no such session.
    pub async fn get_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<Arc<ArcSwap<ActiveSession>>>, String> {
        self.load_or_create(session_id, false).await
    }

    async fn load_or_create(
        &self,
        session_id: Uuid,
        create: bool,
    ) -> Result<Option<Arc<ArcSwap<ActiveSession>>>, String> {
        self.total_session_operations
            .fetch_add(1, Ordering::Relaxed);

        if let Some(session_arc) = self.cached(session_id) {
            self.session_cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(session_arc));
        }
        self.session_cache_misses.fetch_add(1, Ordering::Relaxed);

        // Single flight: the first miss loads, concurrent misses wait for
        // it and then find the session cached. Without this each would
        // install its own handle and writes to the losing one would vanish.
        self.with_load_gate(session_id, async {
            match self.cached(session_id) {
                Some(session_arc) => Ok(Some(session_arc)),
                None => self.load_uncached(session_id, create).await,
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

    /// Load from storage (or create when allowed) and install in the cache.
    /// The caller holds the session's loading gate.
    async fn load_uncached(
        &self,
        session_id: Uuid,
        create: bool,
    ) -> Result<Option<Arc<ArcSwap<ActiveSession>>>, String> {
        // Writes still queued from an evicted copy must land first, or the
        // load would miss them.
        self.storage_actor.wait_persisted(session_id).await;
        let stored = self
            .storage_actor
            .load_session(session_id)
            .await
            .map_err(|e| {
                error!("Storage error loading session {}: {}", session_id, e);
                format!("Storage error: {e}")
            })?;

        let session = match stored {
            Some(session) => session,
            None if create => {
                info!(
                    "Creating new session {} since not found in storage",
                    session_id
                );
                let new_session = ActiveSession::new(session_id, None, None);
                self.storage_actor
                    .save_session(new_session.clone())
                    .await
                    .map_err(|e| format!("Failed to save new session to storage: {e}"))?;
                self.session_count.fetch_add(1, Ordering::Relaxed);
                new_session
            }
            None => return Ok(None),
        };

        Ok(Some(self.install(session_id, session)))
    }

    /// Put a session in the cache as its current handle (registered with
    /// the persister, so older handles of it are superseded) and mark it
    /// active.
    pub fn install(&self, session_id: Uuid, session: ActiveSession) -> Arc<ArcSwap<ActiveSession>> {
        let session_arc = Arc::new(ArcSwap::new(Arc::new(session)));
        self.storage_actor
            .register_session_handle(session_id, &session_arc);
        self.sessions.put(session_id, Arc::clone(&session_arc));
        self.active_sessions
            .insert(session_id, Arc::new(AtomicU64::new(now_secs())));
        session_arc
    }

    /// Get session metrics - all atomic reads
    pub fn get_metrics(&self) -> SessionManagerMetrics {
        let cache_stats = self.sessions.get_stats();

        SessionManagerMetrics {
            session_count: self.session_count.load(Ordering::Relaxed),
            active_sessions: self.active_sessions.len(),
            total_operations: self.total_session_operations.load(Ordering::Relaxed),
            cache_hits: self.session_cache_hits.load(Ordering::Relaxed),
            cache_misses: self.session_cache_misses.load(Ordering::Relaxed),
            cache_hit_rate: cache_stats.hit_rate,
            cache_size: cache_stats.current_size,
            cache_capacity: cache_stats.capacity,
        }
    }
}
