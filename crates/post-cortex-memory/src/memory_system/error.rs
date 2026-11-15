// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Failures of the runtime the capabilities share: storage ports, the
//! session writer and the session lookups. Every capability error carries
//! one of these, so a transport can tell a missing session from a broken
//! store from a corrupt row (ARCHITECTURE rule 5).

use post_cortex_ports::PortError;
use uuid::Uuid;

use crate::memory_system::persister::PersistError;

/// A failure of the shared runtime.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    /// A storage port failed.
    #[error(transparent)]
    Port(#[from] PortError),
    /// A session write did not land.
    #[error(transparent)]
    Persist(#[from] PersistError),
    /// The session does not exist.
    #[error("session {0} not found")]
    SessionNotFound(Uuid),
    /// Other writers kept winning the compare-and-swap of a session.
    #[error("{op}: session {session_id} stayed under contention for every attempt")]
    Contention {
        /// The operation that gave up.
        op: &'static str,
        /// The session.
        session_id: Uuid,
    },
    /// The runtime's configuration is invalid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(&'static str),
    /// Embeddings are on but the embedding engine or search index is missing.
    #[error("embeddings need an embedding engine and a search index")]
    EmbeddingsNotConfigured,
    /// The embedding engine and the search index disagree on the dimension.
    #[error("the embedding engine produces {engine}-dim vectors, the search index holds {index}")]
    DimensionMismatch {
        /// Dimension of the engine's vectors.
        engine: usize,
        /// Dimension of the stored vectors.
        index: usize,
    },
    /// The vectorizer did not start after its retries; the last failure is
    /// in the log and in health.
    #[error("the vectorizer did not start after {attempts} attempts")]
    VectorizerGaveUp {
        /// Failed start attempts.
        attempts: u64,
    },
}
