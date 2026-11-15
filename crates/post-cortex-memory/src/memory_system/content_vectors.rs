// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Dropping the vectors of removed content (records, entities, messages,
//! documents): every capability that removes content calls this one cleanup.

use crate::memory_system::system::MemoryRuntime;
use tracing::warn;
use uuid::Uuid;

impl MemoryRuntime {
    /// Remove the embeddings of deleted content (update ids or
    /// `entity:<name>` of a session, `doc:<id>` of a workspace) of one
    /// source: from the in-memory vector DB, from persistent storage, and
    /// from every cached query they could appear in. Best-effort: failures
    /// are logged, never returned, because the content itself is already
    /// gone.
    pub(crate) async fn drop_content_vectors(&self, session_id: Uuid, content_ids: &[String]) {
        if content_ids.is_empty() {
            return;
        }

        if let Some(vectorizer) = self.content_vectorizer.get() {
            if let Err(e) = vectorizer.remove_contents(session_id, content_ids).await {
                warn!(
                    "Failed to remove {} embedding(s) of session {}: {:#}",
                    content_ids.len(),
                    session_id,
                    e
                );
            }
            return;
        }

        // Vectorizer not initialised: nothing is cached yet; the index
        // deletes the entries with their chunks (session-scoped: entity
        // content ids repeat across sessions).
        if let Some(index) = self.index()
            && let Err(e) = index.delete(&session_id.to_string(), content_ids).await
        {
            warn!(
                "Failed to remove {} embedding(s) of session {}: {:#}",
                content_ids.len(),
                session_id,
                e
            );
        }
    }
}
