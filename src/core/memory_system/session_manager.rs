use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use tracing::{debug, error, warn};
use uuid::Uuid;

use crate::core::cache::SessionCache;
use crate::core::performance::PerformanceMonitor;
use crate::session::active_session::ActiveSession;

use super::metrics::SessionManagerMetrics;
use super::storage_actor::StorageActorHandle;

/// Session manager using `DashMap` and atomic operations
pub struct SessionManager {
    /// Session cache
    pub sessions: SessionCache<Uuid, Arc<ArcSwap<ActiveSession>>>,

    /// Storage communication
    storage_actor: StorageActorHandle,

    /// Performance monitoring
    performance_monitor: Arc<PerformanceMonitor>,

    /// Session metrics - all atomic
    pub session_count: Arc<AtomicUsize>,
    pub total_session_operations: Arc<AtomicU64>,
    pub session_cache_hits: Arc<AtomicU64>,
    pub session_cache_misses: Arc<AtomicU64>,
    pub active_sessions: DashMap<Uuid, Arc<AtomicU64>>, // last_access_timestamp
}

impl SessionManager {
    /// Construct a new session manager wired to the given storage actor.
    pub fn new(
        cache_capacity: usize,
        storage_actor: StorageActorHandle,
        performance_monitor: Arc<PerformanceMonitor>,
    ) -> Result<Self, String> {
        let sessions = SessionCache::new(cache_capacity, "session_cache".to_string())
            .map_err(|e| format!("Failed to create session cache: {e}"))?;
        Ok(Self {
            sessions,
            storage_actor,
            performance_monitor,
            session_count: Arc::new(AtomicUsize::new(0)),
            total_session_operations: Arc::new(AtomicU64::new(0)),
            session_cache_hits: Arc::new(AtomicU64::new(0)),
            session_cache_misses: Arc::new(AtomicU64::new(0)),
            active_sessions: DashMap::new(),
        })
    }

    /// Get or create session via actor communication
    pub async fn get_or_create_session(
        &self,
        session_id: Uuid,
    ) -> Result<Arc<ArcSwap<ActiveSession>>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("get_or_create_session");
        self.total_session_operations
            .fetch_add(1, Ordering::Relaxed);

        // Try cache first
        if let Some(session_arc) = self.sessions.get(&session_id) {
            self.session_cache_hits.fetch_add(1, Ordering::Relaxed);

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if let Some(access_time) = self.active_sessions.get(&session_id) {
                access_time.store(now, Ordering::Relaxed);
            }

            return Ok(session_arc);
        }

        // Cache miss
        self.session_cache_misses.fetch_add(1, Ordering::Relaxed);

        // Try loading from storage via actor. Differentiate between "not found"
        // and actual storage errors to prevent data loss from transient issues.
        let storage_result: Result<Option<ActiveSession>, String> = match self
            .storage_actor
            .load_session(session_id)
            .await
        {
            Ok(Some(session)) => Ok(Some(session)),
            Ok(None) => Ok(None),
            Err(e) => {
                let error_lower = e.to_lowercase();
                if error_lower.contains("not found")
                    || error_lower.contains("does not exist")
                    || error_lower.contains("no such")
                {
                    debug!("Session {} not found in storage: {}", session_id, e);
                    Ok(None)
                } else if error_lower.contains("timeout") {
                    warn!(
                        "Timeout loading session {} from storage: {}. Will create new session.",
                        session_id, e
                    );
                    Ok(None)
                } else {
                    error!(
                        "Storage error loading session {}: {}. Propagating error instead of masking.",
                        session_id, e
                    );
                    Err(format!("Storage error: {}", e))
                }
            }
        };

        match storage_result {
            Ok(Some(session)) => {
                let session_arc = Arc::new(ArcSwap::new(Arc::new(session)));
                self.sessions.put(session_id, Arc::clone(&session_arc));

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.active_sessions
                    .insert(session_id, Arc::new(AtomicU64::new(now)));

                Ok(session_arc)
            }
            Ok(None) => {
                tracing::info!(
                    "Creating new session {} since not found in storage",
                    session_id
                );
                let new_session = ActiveSession::new(session_id, None, None);
                let session_arc = Arc::new(ArcSwap::new(Arc::new(new_session.clone())));

                match self.storage_actor.save_session(new_session).await {
                    Ok(_) => {
                        self.sessions.put(session_id, Arc::clone(&session_arc));

                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        self.active_sessions
                            .insert(session_id, Arc::new(AtomicU64::new(now)));

                        self.session_count.fetch_add(1, Ordering::Relaxed);
                        Ok(session_arc)
                    }
                    Err(e) => Err(format!("Failed to save new session to storage: {e}")),
                }
            }
            Err(e) => Err(format!("Storage error: {e}")),
        }
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
