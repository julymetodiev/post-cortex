// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Sessions: create, get, list, find, update metadata, delete, checkpoint
//! (ARCHITECTURE section 4). Only `create` (and project resolution through
//! it) makes a session.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use post_cortex_ports::SessionChanges;
use post_cortex_ports::SessionCheckpoint;
use post_cortex_ports::SessionSummary;
use tracing::info;
use uuid::Uuid;

use crate::memory_system::MemoryRuntime;

/// Why a session operation failed.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The session does not exist.
    #[error("Session {0} not found")]
    NotFound(Uuid),
    /// The request is malformed.
    #[error("{0}")]
    InvalidArgument(&'static str),
    /// Storage failed.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> SessionError {
    SessionError::Storage(e.into())
}

/// The Sessions capability service.
#[derive(Clone)]
pub struct Sessions {
    rt: Arc<MemoryRuntime>,
    store: Arc<dyn post_cortex_ports::storage::SessionStore>,
}

impl Sessions {
    pub(crate) fn new(
        rt: Arc<MemoryRuntime>,
        store: Arc<dyn post_cortex_ports::storage::SessionStore>,
    ) -> Self {
        Self { rt, store }
    }

    /// Create a session (stored before it is returned). A missing name
    /// becomes `Session <first id block>`, a missing description
    /// `New conversation session`.
    ///
    /// # Errors
    /// Storage failed; nothing is left behind.
    pub async fn create(
        &self,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<Uuid, SessionError> {
        let rt = &self.rt;
        rt.system_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);
        let session_id = Uuid::new_v4();
        let name = name.or_else(|| {
            let block = session_id.to_string();
            let first = block.split('-').next().unwrap_or("unknown").to_string();
            Some(format!("Session {first}"))
        });
        let description = description.or_else(|| Some("New conversation session".to_string()));
        let session = ActiveSession::new(session_id, name, description, chrono::Utc::now());

        // Stored first; cached only once storage has it.
        let saved = rt
            .persister
            .exclusive(
                session_id,
                self.store
                    .save_session(&session, SessionChanges::everything(&session, &[])),
            )
            .await;
        match saved {
            Ok(_) => {
                rt.session_manager
                    .install(session_id, StoredSession::new(session, Vec::new()));
                rt.session_manager
                    .session_count
                    .fetch_add(1, Ordering::Relaxed);
                rt.system_metrics
                    .successful_requests
                    .fetch_add(1, Ordering::Relaxed);
                info!("Created new session: {}", session_id);
                Ok(session_id)
            }
            Err(e) => {
                rt.system_metrics
                    .failed_requests
                    .fetch_add(1, Ordering::Relaxed);
                Err(storage(e))
            }
        }
    }

    /// The current snapshot of a session, loaded into the cache if needed.
    ///
    /// # Errors
    /// `NotFound`, or the session could not be loaded.
    pub async fn get(&self, session_id: Uuid) -> Result<Arc<ActiveSession>, SessionError> {
        self.rt
            .session_manager
            .get_session(session_id)
            .await
            .map_err(SessionError::Storage)?
            .map(|handle| handle.load_full())
            .ok_or(SessionError::NotFound(session_id))
    }

    /// How many records of a session are waiting for their embedding (in
    /// the session, not yet in the vector index); loads it if needed.
    ///
    /// # Errors
    /// `NotFound`, or the session could not be loaded.
    pub async fn pending_vectorization_count(
        &self,
        session_id: Uuid,
    ) -> Result<usize, SessionError> {
        let handle = self
            .rt
            .session_manager
            .get_session(session_id)
            .await
            .map_err(SessionError::Storage)?
            .ok_or(SessionError::NotFound(session_id))?;
        Ok(handle.vectorized().pending_count(&handle.load()))
    }

    /// The snapshot of a session if it is in the cache; never loads.
    #[must_use]
    pub fn cached(&self, session_id: Uuid) -> Option<Arc<ActiveSession>> {
        self.rt
            .session_manager
            .cached(session_id)
            .map(|handle| handle.load_full())
    }

    /// Every stored session id.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn list(&self) -> Result<Vec<Uuid>, SessionError> {
        self.rt.list_sessions().await.map_err(SessionError::Storage)
    }

    /// Metadata of every stored session, oldest first, without loading any.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn list_summaries(&self) -> Result<Vec<SessionSummary>, SessionError> {
        let rt = &self.rt;
        let mut summaries = rt
            .stores
            .sessions
            .list_session_summaries()
            .await
            .map_err(storage)?;
        // A cached session reports its in-memory state, which may be ahead
        // of the last write.
        for summary in &mut summaries {
            if let Some(handle) = rt.session_manager.sessions.peek(&summary.id) {
                *summary = SessionSummary::from_session(&handle.load());
            }
        }
        summaries.sort_by_key(|s| (s.created_at, s.id));
        Ok(summaries)
    }

    /// Metadata of the sessions whose name or description contains `query`
    /// (case-insensitive).
    ///
    /// # Errors
    /// Storage failed.
    pub async fn find(&self, query: &str) -> Result<Vec<SessionSummary>, SessionError> {
        if query.trim().is_empty() {
            return Err(SessionError::InvalidArgument("query cannot be empty"));
        }
        let query = query.to_lowercase();
        let matches = |text: &Option<String>| {
            text.as_ref()
                .is_some_and(|t| t.to_lowercase().contains(&query))
        };
        let mut summaries = self.list_summaries().await?;
        summaries.retain(|s| matches(&s.name) || matches(&s.description));
        Ok(summaries)
    }

    /// Change a session's name and/or description.
    ///
    /// # Errors
    /// The session does not exist or the change could not be swapped in.
    pub async fn update_metadata(
        &self,
        session_id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<(), SessionError> {
        if name.is_none() && description.is_none() {
            return Err(SessionError::InvalidArgument(
                "update_metadata requires at least one of name or description",
            ));
        }
        let handle = self
            .rt
            .session_manager
            .get_session(session_id)
            .await
            .map_err(SessionError::Storage)?
            .ok_or(SessionError::NotFound(session_id))?;
        let now = chrono::Utc::now();
        let updated = MemoryRuntime::cas_update_session(
            &handle,
            session_id,
            "update_session_metadata",
            |session| {
                session.update_metadata(name.clone(), description.clone(), now);
                true
            },
        )
        .await
        .map_err(SessionError::Storage)?;
        if updated.is_some() {
            self.rt.persister.persist_nowait(&handle, vec![]);
        }
        Ok(())
    }

    /// Delete a session, its records, vectors and checkpoints; returns
    /// whether it was deleted. The session leaves the cache under its
    /// loading gate (a load cannot read it before the delete and cache it
    /// after), its queued persists are dropped and every handle loaded so
    /// far is retired, so no late write brings it back.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn delete(&self, session_id: Uuid) -> Result<bool, SessionError> {
        let rt = &self.rt;
        let deleted = rt
            .session_manager
            .with_load_gate(session_id, async {
                let live = rt.session_manager.forget(&session_id);
                rt.persister
                    .delete(session_id, live.as_ref())
                    .await
                    .map_err(storage)
            })
            .await?;
        if let Some(vectorizer) = rt.content_vectorizer.get()
            && let Err(e) = vectorizer.remove_session_vectors(session_id).await
        {
            tracing::warn!("Vectors of deleted session {session_id} kept: {e:#}");
        }
        Ok(deleted)
    }

    /// Store a checkpoint of the session as it is now; returns its id.
    ///
    /// # Errors
    /// The session does not exist or storage failed.
    pub async fn checkpoint(&self, session_id: Uuid) -> Result<Uuid, SessionError> {
        let session = self.get(session_id).await?;
        let checkpoint = SessionCheckpoint {
            id: Uuid::new_v4(),
            session_id,
            created_at: chrono::Utc::now(),
            structured_context: session.current_state(),
            recent_updates: session.records_owned(),
            code_references: session.code_references().into_iter().collect(),
            change_history: session.change_history(),
            total_updates: session.records().len(),
            context_quality_score: 1.0,
            compression_ratio: 1.0,
        };
        self.rt
            .stores
            .checkpoints
            .save_checkpoint(&checkpoint)
            .await
            .map_err(storage)?;
        Ok(checkpoint.id)
    }
}
