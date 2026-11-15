// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding captured messages: chunked, and only what is missing.

use crate::content_vectorizer::ContentVectorizer;
use crate::content_vectorizer::ingestion::entries;
use crate::content_vectorizer::vector_text::message_vectors;
use post_cortex_core::core::message::{self, Message};
use post_cortex_ports::error::Result;
use post_cortex_ports::{EntrySelect, VectorMetadata};
use std::collections::{HashMap, HashSet};
use tracing::debug;
use uuid::Uuid;

impl ContentVectorizer {
    /// Vectorize turns of a session's message stream (see
    /// `message_vectors`). A turn whose stored vectors are exactly the ones
    /// its text gives (the same chunk ids with the same texts) is skipped;
    /// any other stored vectors of it (chunks missing or left over, or the
    /// vectors of another text at that position) are replaced. A turn too
    /// short to embed keeps no vectors. Returns how many vectors were added.
    ///
    /// # Errors
    /// Returns an error if the texts cannot be embedded or stale vectors
    /// cannot be deleted.
    pub async fn vectorize_messages(&self, messages: &[Message]) -> Result<usize> {
        let mut vectors: Vec<(String, VectorMetadata)> = Vec::new();
        // Stale vectors, by session: their content ids.
        let mut stale: HashMap<Uuid, HashSet<String>> = HashMap::new();
        for m in messages {
            let wanted = if crate::content_vectorizer::vector_text::worth_embedding(self, &m.text) {
                message_vectors(m, self.config().max_text_length)
            } else {
                Vec::new()
            };
            let stored = self.stored_message_vectors(m).await?;
            let complete = stored.len() == wanted.len()
                && wanted.iter().all(|(text, meta)| {
                    stored.iter().any(|s| s.id == meta.id && *s.text == **text)
                });
            if complete {
                continue;
            }
            if !stored.is_empty() {
                let content_ids = stale.entry(m.session_id).or_default();
                for meta in stored {
                    content_ids.insert(meta.id);
                }
            }
            vectors.extend(wanted);
        }
        let embeddings = if vectors.is_empty() {
            Vec::new()
        } else {
            let texts: Vec<String> = vectors.iter().map(|(text, _)| text.clone()).collect();
            let embeddings = self.embedding_engine().embed(texts).await?;
            if embeddings.len() != vectors.len() {
                return Err(post_cortex_ports::PortError::unavailable(format!(
                    "the embedding engine returned {} vectors for {} message texts",
                    embeddings.len(),
                    vectors.len()
                )));
            }
            embeddings
        };
        // Embedded first, so a failed embedding leaves the old vectors.
        for (session_id, content_ids) in &stale {
            let content_ids: Vec<String> = content_ids.iter().cloned().collect();
            let removed = self.remove_contents(*session_id, &content_ids).await?;
            debug!(
                "Replacing {} stale message vectors of session {}",
                removed, session_id
            );
        }
        if vectors.is_empty() {
            return Ok(0);
        }
        let metadata = vectors.into_iter().map(|(_, meta)| meta);
        let added = self
            .store_vectors(entries(embeddings, metadata.collect()))
            .await;
        let mut sessions: Vec<Uuid> = messages.iter().map(|m| m.session_id).collect();
        sessions.dedup();
        for session_id in sessions {
            self.invalidate_cached(session_id);
        }
        Ok(added.len())
    }

    /// The stored vectors of message `m`: `message:<seq>` and its chunks.
    async fn stored_message_vectors(&self, m: &Message) -> Result<Vec<VectorMetadata>> {
        let select = EntrySelect::ContentIds(vec![message::content_id(m.seq)]);
        Ok(self
            .index()
            .entries(&m.session_id.to_string(), &select)
            .await?
            .into_iter()
            .map(|e| e.meta)
            .collect())
    }
}
