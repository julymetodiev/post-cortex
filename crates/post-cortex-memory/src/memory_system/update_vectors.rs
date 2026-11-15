// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding written records and the entities they name (the derived work
//! of writes), a batch at a time, and undoing it for a session deleted
//! meanwhile.

use crate::memory_system::{MemoryRuntime, RuntimeError};
use post_cortex_core::core::context_update::ContextUpdate;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, warn};
use uuid::Uuid;

/// Remove what vectorizing `update_ids` added for a session that was
/// deleted meanwhile: every entry of the session left in the index (the
/// records' and the entities' vectors), in one delete, and cached results.
pub async fn undo_vectorization(
    vectorizer: &crate::content_vectorizer::ContentVectorizer,
    session_id: Uuid,
    update_ids: &[Uuid],
) {
    if let Err(e) = vectorizer.remove_session_vectors(session_id).await {
        warn!(
            "Failed to remove the embeddings of {} record(s) of deleted session {}: {:#}",
            update_ids.len(),
            session_id,
            e
        );
    }
}

/// The entity names `updates` created, referenced or related, deduplicated.
fn entity_names(updates: &[Arc<ContextUpdate>]) -> HashSet<String> {
    updates
        .iter()
        .flat_map(|update| {
            update
                .creates_entities
                .iter()
                .chain(&update.references_entities)
                .chain(update.typed_entities.iter().map(|e| &e.name))
                .chain(
                    update
                        .creates_relationships
                        .iter()
                        .flat_map(|r| [&r.from_entity, &r.to_entity]),
                )
        })
        .cloned()
        .collect()
}

impl MemoryRuntime {
    /// Embed the descriptions of the entities `updates` created, referenced
    /// or related, now that they are in the graph (new or changed ones only,
    /// each once however many of the updates name it), so they are found
    /// without a later `vectorize_session`. Failures are logged: the graph
    /// merge itself has succeeded.
    async fn vectorize_update_entities(
        &self,
        session_id: Uuid,
        session_arc: &crate::memory_system::SessionHandle,
        updates: &[Arc<ContextUpdate>],
    ) {
        let names = entity_names(updates);
        if names.is_empty() {
            return;
        }
        let vectorizer = match self.ensure_vectorizer_initialized().await {
            Ok(v) => v,
            Err(e) => {
                warn!("Entities of session {session_id} not embedded: {e}");
                return;
            }
        };
        let session = session_arc.load_full();
        match vectorizer.vectorize_entities(&session, Some(&names)).await {
            Ok((added, _)) if added > 0 && self.persister.is_deleted(session_id, session_arc) => {
                // Deleted while embedding: the vectors must not outlive it.
                let content_ids: Vec<String> =
                    names.iter().map(|name| format!("entity:{name}")).collect();
                if let Err(e) = vectorizer.remove_contents(session_id, &content_ids).await {
                    warn!("Entity vectors of deleted session {session_id} kept: {e:#}");
                }
            }
            Ok(_) => {}
            Err(e) => warn!("Entities of session {session_id} not embedded: {e:#}"),
        }
    }

    /// [`Self::vectorize_updates_now`] for one record.
    pub async fn vectorize_update_now(
        &self,
        session_id: Uuid,
        update_id: Uuid,
    ) -> Result<usize, RuntimeError> {
        self.vectorize_updates_now(session_id, &[update_id]).await
    }

    /// Embed records of one session and the entities they name **now** (no
    /// spawn): the derived work of writes, run by the
    /// [`crate::pipeline::EmbeddingQueue`] workers a batch at a time (or by
    /// a writer itself when the queue stays full). One `encode_batch` and
    /// one vector store batch for the records, one session persist, one
    /// pass over the entities they name. Takes the update ids: the queue may
    /// run after later writes landed. Skips sessions that no longer exist.
    /// Returns how many record vectors were added.
    ///
    /// # Errors
    /// Returns an error if the vectorizer cannot be initialised or the
    /// records cannot be embedded.
    pub async fn vectorize_updates_now(
        &self,
        session_id: Uuid,
        update_ids: &[Uuid],
    ) -> Result<usize, RuntimeError> {
        if !self.config.enable_embeddings
            || !self.config.auto_vectorize_on_update
            || update_ids.is_empty()
        {
            return Ok(0);
        }

        let Some(session_arc) = self.session_manager.get_session(session_id).await? else {
            debug!(
                "vectorize_updates_now: session {} not found, skipping",
                session_id
            );
            return Ok(0);
        };

        let vectorizer = self.ensure_vectorizer_initialized().await?;
        let added = self
            .vectorize_and_commit(
                &vectorizer,
                session_id,
                &session_arc,
                crate::memory_system::embeddings::VectorizeTarget::Updates(update_ids),
            )
            .await?;
        let wanted: HashSet<&Uuid> = update_ids.iter().collect();
        let updates: Vec<Arc<ContextUpdate>> = session_arc
            .load()
            .records()
            .iter()
            .filter(|u| wanted.contains(&u.id))
            .cloned()
            .collect();
        self.vectorize_update_entities(session_id, &session_arc, &updates)
            .await;
        Ok(added)
    }
}
