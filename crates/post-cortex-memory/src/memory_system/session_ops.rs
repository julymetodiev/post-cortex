// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The session lookups every capability shares: one session's handle and
//! the ids of every stored session.

use crate::memory_system::{MemoryRuntime, RuntimeError, SessionHandle};
use std::sync::atomic::Ordering;
use uuid::Uuid;

impl MemoryRuntime {
    /// The handle of an existing session, loaded if needed.
    pub async fn get_session(&self, session_id: Uuid) -> Result<SessionHandle, RuntimeError> {
        self.session_manager
            .get_session(session_id)
            .await?
            .ok_or(RuntimeError::SessionNotFound(session_id))
    }

    /// List all sessions
    pub async fn list_sessions(&self) -> Result<Vec<Uuid>, RuntimeError> {
        match self.stores.sessions.list_sessions().await {
            Ok(session_ids) => {
                self.system_metrics
                    .successful_requests
                    .fetch_add(1, Ordering::Relaxed);
                Ok(session_ids)
            }
            Err(e) => {
                self.system_metrics
                    .failed_requests
                    .fetch_add(1, Ordering::Relaxed);
                Err(e.into())
            }
        }
    }
}
