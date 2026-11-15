// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Where the session writer writes, and why a write did not land.

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::{PortError, SessionChanges, SessionStore};
use uuid::Uuid;

/// Why a persist request was not written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersistError {
    /// The storage write failed; the rows are retried by the next write.
    #[error("session write failed: {0}")]
    Write(#[from] PortError),
    /// The storage adapter panicked during the write.
    #[error("session write panicked in the storage adapter")]
    Panicked,
    /// The session was deleted before the write.
    #[error("session was deleted")]
    SessionDeleted,
    /// The handle was superseded by a newer load and no live handle is left.
    #[error("session handle was superseded by a newer load")]
    HandleSuperseded,
    /// The writer stopped before answering.
    #[error("the session writer stopped before answering")]
    WriterGone,
}

/// Where [`SessionPersister`] writes. Implemented for the session port; a
/// seam so ordering and coalescing can be tested.
#[async_trait]
pub trait PersistSink: Send + Sync {
    /// Write `session` with `changes` (see `SessionStore::save_session`).
    async fn write(
        &self,
        session: &ActiveSession,
        changes: SessionChanges,
    ) -> Result<(), PortError>;
    /// Delete the session and everything stored for it.
    async fn delete(&self, session_id: Uuid) -> Result<bool, PortError>;
}

/// [`PersistSink`] over the session port.
pub struct StorageSink(Arc<dyn SessionStore>);

impl StorageSink {
    /// A sink writing through `sessions`.
    pub fn new(sessions: Arc<dyn SessionStore>) -> Self {
        Self(sessions)
    }
}

#[async_trait]
impl PersistSink for StorageSink {
    async fn write(
        &self,
        session: &ActiveSession,
        changes: SessionChanges,
    ) -> Result<(), PortError> {
        self.0.save_session(session, changes).await
    }

    async fn delete(&self, session_id: Uuid) -> Result<bool, PortError> {
        self.0.delete_session(session_id).await
    }
}
