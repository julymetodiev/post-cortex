// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Typed error hierarchy for `post-cortex-memory`.
//!
//! Composes the lower-layer errors from `post-cortex-storage` and
//! `post-cortex-embeddings` plus orchestrator-specific variants
//! (pipeline backpressure, circuit breaker open, session not found).

use thiserror::Error;
use uuid::Uuid;

/// Errors produced by the orchestrator layer.
#[derive(Debug, Error)]
pub enum Error {
    /// Underlying storage error.
    #[error(transparent)]
    Storage(#[from] post_cortex_storage::error::Error),

    /// Underlying embeddings / vector-db error.
    #[error(transparent)]
    Embeddings(#[from] post_cortex_embeddings::error::Error),

    /// Domain-level error from `post-cortex-core`.
    #[error(transparent)]
    Domain(#[from] post_cortex_core::SystemError),

    /// Session not found in the active cache or backend.
    #[error("session not found: {0}")]
    SessionNotFound(Uuid),

    /// Circuit breaker is open, request rejected without trying the
    /// underlying operation.
    #[error("circuit breaker open: {0}")]
    CircuitBreaker(String),

    /// Pipeline applied backpressure — the bounded queue was full.
    #[error("pipeline backpressure on {queue}")]
    Backpressure {
        /// Name of the saturated queue.
        queue: &'static str,
    },

    /// Pipeline worker shut down before the work could be processed.
    #[error("pipeline worker shut down: {queue}")]
    WorkerShutdown {
        /// Name of the shut-down queue.
        queue: &'static str,
    },

    /// Operation timed out.
    #[error("operation timeout after {ms}ms")]
    Timeout {
        /// Elapsed wall-clock time in milliseconds.
        ms: u64,
    },

    /// Catch-all for migrating call sites that still use `anyhow`.
    #[error(transparent)]
    External(#[from] anyhow::Error),
}

/// Crate-level result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<crate::pipeline::PipelineError> for Error {
    fn from(err: crate::pipeline::PipelineError) -> Self {
        match err {
            crate::pipeline::PipelineError::Backpressure { queue } => Self::Backpressure { queue },
            crate::pipeline::PipelineError::WorkerShutdown { queue } => {
                Self::WorkerShutdown { queue }
            }
        }
    }
}

impl Error {
    /// A stable discriminant for metrics / structured logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Storage(_) => "storage",
            Self::Embeddings(_) => "embeddings",
            Self::Domain(_) => "domain",
            Self::SessionNotFound(_) => "session_not_found",
            Self::CircuitBreaker(_) => "circuit_breaker",
            Self::Backpressure { .. } => "backpressure",
            Self::WorkerShutdown { .. } => "worker_shutdown",
            Self::Timeout { .. } => "timeout",
            Self::External(_) => "external",
        }
    }
}
