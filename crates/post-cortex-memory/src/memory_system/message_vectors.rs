// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The vectors of captured messages: embedding queued turns (the derived
//! work of an append) and repairing a session's message vectors. Shared by
//! the pipeline, Admin and Transfer.

use std::collections::HashSet;

use tracing::{info, warn};
use uuid::Uuid;

use crate::memory_system::RuntimeError;
use crate::memory_system::system::MemoryRuntime;

/// Messages embedded per call when a session's stream is backfilled.
const MESSAGE_EMBED_BATCH: usize = 256;

impl MemoryRuntime {
    /// Embed the stored turns of `session_id` at `seqs` **now**: the
    /// derived work of an append, run by the embedding pipeline. Turns no
    /// longer stored are skipped; if the session was deleted meanwhile, the
    /// vectors added are removed again. Returns how many vectors were
    /// added.
    ///
    /// # Errors
    /// The turns could not be read, the vectorizer not started or the turns
    /// not embedded.
    pub(crate) async fn vectorize_messages_now(
        &self,
        session_id: Uuid,
        seqs: &[u64],
    ) -> Result<usize, RuntimeError> {
        let (Some(&first), Some(&last)) = (seqs.iter().min(), seqs.iter().max()) else {
            return Ok(0);
        };
        if !self.config.enable_embeddings {
            return Ok(0);
        }
        let wanted: HashSet<u64> = seqs.iter().copied().collect();
        let mut messages = self
            .stores
            .messages
            .load_messages(session_id, first..last.saturating_add(1))
            .await?;
        messages.retain(|m| wanted.contains(&m.seq));
        if messages.is_empty() {
            return Ok(0);
        }
        let vectorizer = self.ensure_vectorizer_initialized().await?;
        let added = vectorizer.vectorize_messages(&messages).await?;
        let exists = self.session_manager.cached(session_id).is_some()
            || match self.stores.sessions.session_exists(session_id).await {
                Ok(exists) => exists,
                Err(e) => {
                    // Keep the vectors: a session that cannot be checked
                    // is not known to be deleted.
                    warn!("Session {session_id} not checked after embedding its turns: {e}");
                    true
                }
            };
        if added > 0 && !exists {
            // Deleted while embedding: the vectors must not outlive it.
            let positions: HashSet<u64> = messages.iter().map(|m| m.seq).collect();
            if let Err(e) = vectorizer
                .remove_message_vectors(session_id, &positions)
                .await
            {
                warn!("Message vectors of deleted session {session_id} kept: {e:#}");
            }
            return Ok(0);
        }
        Ok(added)
    }

    /// Bring the message vectors of `session_id` in step with its stream:
    /// vectors of positions no stored message has (left by an older import
    /// or a delete that missed chunks) and chunk sets with gaps are removed,
    /// and every message whose vectors are not exactly those of its text
    /// (none yet, chunks missing, another text's) is embedded again.
    /// Returns how many vectors were added.
    ///
    /// # Errors
    /// The stream could not be read, the vectorizer not started, or the
    /// vectors not embedded or deleted.
    pub(crate) async fn vectorize_session_messages(
        &self,
        session_id: Uuid,
    ) -> Result<usize, RuntimeError> {
        let vectorizer = self.ensure_vectorizer_initialized().await?;
        // Vectors before messages: a message is stored before its vectors,
        // so a vector seen here whose message is not loaded below has none.
        let layout = vectorizer.message_vector_chunks(session_id).await?;
        let messages = self
            .stores
            .messages
            .load_messages(session_id, 0..u64::MAX)
            .await?;
        let stored: HashSet<u64> = messages.iter().map(|m| m.seq).collect();
        let broken: HashSet<u64> = layout
            .into_iter()
            .filter(|(seq, chunks)| !stored.contains(seq) || !is_chunk_run(chunks))
            .map(|(seq, _)| seq)
            .collect();
        if !broken.is_empty() {
            let removed = vectorizer
                .remove_message_vectors(session_id, &broken)
                .await?;
            info!(
                "Removed {} stale message vectors ({} positions) of session {}",
                removed,
                broken.len(),
                session_id
            );
        }
        let mut added = 0;
        for batch in messages.chunks(MESSAGE_EMBED_BATCH) {
            added += vectorizer.vectorize_messages(batch).await?;
        }
        if added > 0 {
            info!(
                "Embedded {} message vectors of session {}",
                added, session_id
            );
        }
        Ok(added)
    }
}

/// Whether `chunks` (chunk numbers of one message's vectors) are `1..=n`,
/// each once.
fn is_chunk_run(chunks: &[u32]) -> bool {
    let mut sorted = chunks.to_vec();
    sorted.sort_unstable();
    sorted.iter().zip(1u32..).all(|(&chunk, n)| chunk == n)
}
