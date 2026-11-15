// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Entities: delete and merge entities of a session's graph (ARCHITECTURE
//! section 4).

use std::sync::Arc;

use uuid::Uuid;

use crate::memory_system::{MemoryRuntime, SessionHandle};

/// Why an entity operation failed.
#[derive(Debug, thiserror::Error)]
pub enum EntityError {
    /// The session does not exist.
    #[error("Session {0} not found")]
    SessionNotFound(Uuid),
    /// The request is malformed.
    #[error("{0}")]
    InvalidArgument(&'static str),
    /// The change could not be swapped in, or storage failed.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// The Entities capability service.
#[derive(Clone)]
pub struct Entities {
    rt: Arc<MemoryRuntime>,
}

impl Entities {
    pub(crate) fn new(rt: Arc<MemoryRuntime>) -> Self {
        Self { rt }
    }

    /// Delete an entity and its edges from a session's graph: the node and
    /// its edges leave the in-memory graph (one compare-and-swap, so
    /// concurrent writes neither lose nor resurrect it), the session is
    /// persisted with its whole graph (the entity's stored row and edges
    /// go) and the entity's embedding is dropped. Returns whether it
    /// existed.
    ///
    /// # Errors
    /// `SessionNotFound`, contention or storage failed.
    pub async fn delete(&self, session_id: Uuid, name: &str) -> Result<bool, EntityError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EntityError::InvalidArgument("entity_name cannot be empty"));
        }
        let handle = self.handle(session_id).await?;
        let updated =
            MemoryRuntime::cas_update_session(&handle, session_id, "delete_entity", |session| {
                if !session.entity_graph().has_entity(name) {
                    return false;
                }
                session.entity_graph_mut().remove_entity(name);
                true
            })
            .await
            .map_err(EntityError::Storage)?;
        let existed = updated.is_some();
        if existed {
            self.rt.persister.persist_graph_nowait(&handle);
            self.drop_vector(session_id, name).await;
        }
        Ok(existed)
    }

    /// Merge entity `alias` into `into` in a session's graph: its
    /// mentions, relations and counts move over and its name resolves to
    /// `into` from then on; the session is persisted with its whole graph
    /// and the alias's embedding is dropped.
    /// Returns false if either entity is unknown or both are one.
    ///
    /// # Errors
    /// `SessionNotFound`, contention or storage failed.
    pub async fn merge(
        &self,
        session_id: Uuid,
        alias: &str,
        into: &str,
    ) -> Result<bool, EntityError> {
        let (alias, into) = (alias.trim(), into.trim());
        if alias.is_empty() || into.is_empty() {
            return Err(EntityError::InvalidArgument(
                "entity_name and into cannot be empty",
            ));
        }
        let handle = self.handle(session_id).await?;
        let mut stored_alias = None;
        let updated =
            MemoryRuntime::cas_update_session(&handle, session_id, "merge_entities", |session| {
                let Some(name) = session.entity_graph().resolve(alias).map(str::to_string) else {
                    return false;
                };
                if !session.entity_graph_mut().merge_entities(alias, into) {
                    return false;
                }
                stored_alias = Some(name);
                true
            })
            .await
            .map_err(EntityError::Storage)?;
        if updated.is_none() {
            return Ok(false);
        }
        self.rt.persister.persist_graph_nowait(&handle);
        if let Some(name) = stored_alias {
            self.drop_vector(session_id, &name).await;
        }
        Ok(true)
    }

    /// Drop the embedding of the entity `name`, which left the graph.
    async fn drop_vector(&self, session_id: Uuid, name: &str) {
        self.rt
            .drop_content_vectors(session_id, &[format!("entity:{name}")])
            .await;
    }

    async fn handle(&self, session_id: Uuid) -> Result<SessionHandle, EntityError> {
        self.rt
            .session_manager
            .get_session(session_id)
            .await
            .map_err(EntityError::Storage)?
            .ok_or(EntityError::SessionNotFound(session_id))
    }
}
