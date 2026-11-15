// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Admin: health, vectorization passes and their stats, the query cache,
//! warm-up and the shutdown flush (ARCHITECTURE section 4).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::memory_system::MemoryRuntime;
use crate::pipeline::Pipeline;

mod vectorize;

/// Why an admin operation failed.
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    /// The vectorizer could not start, or storage failed.
    #[error(transparent)]
    Runtime(#[from] crate::memory_system::RuntimeError),
    /// No vectorizer has started yet (nothing has been embedded or
    /// searched since the process started).
    #[error("embeddings not initialized yet (call any vectorization method first)")]
    EmbeddingsNotStarted,
}

impl From<post_cortex_ports::PortError> for AdminError {
    fn from(e: post_cortex_ports::PortError) -> Self {
        AdminError::Runtime(e.into())
    }
}

/// Counts of a pass over every session: (total, succeeded, failed).
pub type PassCounts = (usize, usize, usize);

/// The health of one memory instance: the one payload every transport
/// (MCP `admin health`, gRPC `Health`, REST `/health`) reports.
#[derive(Debug, Clone, Serialize)]
pub struct Health {
    /// No session's last write failed (writes reach storage).
    pub healthy: bool,
    /// The memory crate's version.
    pub version: &'static str,
    /// Sessions in the cache.
    pub active_sessions: usize,
    /// Requests counted since start (record writes and session creates).
    pub total_requests: u64,
    /// Of those, the ones that succeeded.
    pub successful_requests: u64,
    /// Of those, the ones that failed.
    pub failed_requests: u64,
    /// Operations running now.
    pub active_operations: usize,
    /// Session cache hit rate (0 to 1).
    pub cache_hit_rate: f64,
    /// Seconds since the instance started.
    pub uptime_seconds: u64,
    /// Embeddings are on and the vectorizer is initialised.
    pub embeddings_enabled: bool,
    /// Embedding work items queued on the pipeline.
    pub pipeline_backlog: usize,
    /// What the vectorizer loaded from storage when it started (absent
    /// before it started).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_load: Option<post_cortex_ports::LoadReport>,
}

/// The Admin capability service.
#[derive(Clone)]
pub struct Admin {
    rt: Arc<MemoryRuntime>,
    pipeline: Arc<Pipeline>,
}

impl Admin {
    pub(crate) fn new(rt: Arc<MemoryRuntime>, pipeline: Arc<Pipeline>) -> Self {
        Self { rt, pipeline }
    }

    /// The health of the instance (all atomic reads).
    #[must_use]
    pub fn health(&self) -> Health {
        let rt = &self.rt;
        let cache = rt.session_manager.sessions.stats();
        let metrics = &rt.system_metrics;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Health {
            healthy: rt.persister.failing() == 0,
            version: env!("CARGO_PKG_VERSION"),
            active_sessions: cache.size,
            total_requests: metrics.total_requests.load(Ordering::Relaxed),
            successful_requests: metrics.successful_requests.load(Ordering::Relaxed),
            failed_requests: metrics.failed_requests.load(Ordering::Relaxed),
            active_operations: metrics.active_operations.load(Ordering::Relaxed),
            cache_hit_rate: cache.hit_rate(),
            uptime_seconds: now.saturating_sub(metrics.start_timestamp.load(Ordering::Relaxed)),
            embeddings_enabled: self.embeddings_enabled(),
            pipeline_backlog: self.pipeline.backlog(),
            vector_load: self.vector_load(),
        }
    }

    /// Whether embeddings are on and the vectorizer is initialised.
    #[must_use]
    pub fn embeddings_enabled(&self) -> bool {
        self.rt.config.enable_embeddings && self.rt.content_vectorizer.get().is_some()
    }

    /// Drop cached query results (stale after a restart).
    pub fn clear_query_cache(&self) {
        if let Some(vectorizer) = self.rt.content_vectorizer.get() {
            vectorizer.clear_query_cache();
            tracing::info!("Query cache cleared");
        }
    }

    /// Finish queued embeddings, then write every queued session persist,
    /// waiting at most `limit`; returns how many sessions could not be
    /// written. Call before a graceful shutdown.
    pub async fn flush_all(&self, limit: Duration) -> usize {
        self.rt.flush_all(limit).await
    }

    fn vector_load(&self) -> Option<post_cortex_ports::LoadReport> {
        self.rt.content_vectorizer.get()?.load_report()
    }
}
