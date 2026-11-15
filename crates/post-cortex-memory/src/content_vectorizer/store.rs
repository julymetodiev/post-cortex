// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Removing a session's vectors: whole sessions, content items with their
//! chunks, and message positions.

use post_cortex_core::core::message;
use post_cortex_ports::error::Result;
use post_cortex_ports::{EntrySelect, MESSAGE_CONTENT_TYPE};
use std::collections::{HashMap, HashSet};
use tracing::debug;
use uuid::Uuid;

use crate::content_vectorizer::ContentVectorizer;

impl ContentVectorizer {
    /// Remove every vector of a session (e.g. on session delete) and
    /// invalidate the query cache entries it affects. Returns how many
    /// vectors were removed.
    ///
    /// # Errors
    /// The index could not be read or the vectors not deleted.
    pub async fn remove_session_vectors(&self, session_id: Uuid) -> Result<usize> {
        let source = session_id.to_string();
        let content_ids: Vec<String> = self
            .index()
            .entries(&source, &EntrySelect::All)
            .await?
            .into_iter()
            .map(|e| e.meta.id)
            .collect();
        self.forget_vectorized(session_id);
        let removed = self.remove_stored_vectors(session_id, &content_ids).await?;
        debug!("Removed {} vectors of session {}", removed, session_id);
        Ok(removed)
    }

    /// Remove the vectors of content items (update ids, or `entity:<name>`)
    /// belonging to `session_id`, with the chunks of long ones (`<id>#2`,
    /// `<id>#3`, ...), in one batch, and invalidate affected cached queries.
    ///
    /// Only that session's copies are touched: entity content ids are not
    /// unique across sessions. Returns how many vectors were removed.
    ///
    /// # Errors
    /// Returns an error if the vectors could not be deleted.
    pub async fn remove_contents(&self, session_id: Uuid, content_ids: &[String]) -> Result<usize> {
        self.remove_stored_vectors(session_id, content_ids).await
    }

    /// The message vectors of `session_id`, by message position: the chunk
    /// number of each (1 for `message:<seq>`, `n` for `message:<seq>#<n>`;
    /// 0 for a suffix that is not a number), in no particular order.
    ///
    /// # Errors
    /// The index could not be read.
    pub async fn message_vector_chunks(&self, session_id: Uuid) -> Result<HashMap<u64, Vec<u32>>> {
        let mut layout: HashMap<u64, Vec<u32>> = HashMap::new();
        for entry in self.message_entries(session_id).await? {
            let Some(seq) = message::seq_of_content_id(&entry.meta.id) else {
                continue;
            };
            let chunk = match entry.meta.id.split_once('#') {
                None => 1,
                Some((_, n)) => n.parse().map_err(|_| {
                    post_cortex_ports::error::corrupt("index entry", &entry.meta.id, "chunk number")
                })?,
            };
            layout.entry(seq).or_default().push(chunk);
        }
        Ok(layout)
    }

    /// Remove every vector (each chunk) of the messages of `session_id` at
    /// `seqs`, and invalidate the session's cached queries. Returns how many
    /// vectors were removed.
    ///
    /// # Errors
    /// Returns an error if the vectors could not be read or deleted.
    pub async fn remove_message_vectors(
        &self,
        session_id: Uuid,
        seqs: &HashSet<u64>,
    ) -> Result<usize> {
        if seqs.is_empty() {
            return Ok(0);
        }
        let content_ids: Vec<String> = self
            .message_entries(session_id)
            .await?
            .into_iter()
            .filter(|e| message::seq_of_content_id(&e.meta.id).is_some_and(|s| seqs.contains(&s)))
            .map(|e| e.meta.id)
            .collect();
        self.remove_stored_vectors(session_id, &content_ids).await
    }

    async fn message_entries(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<post_cortex_ports::IndexEntry>> {
        self.index()
            .entries(
                &session_id.to_string(),
                &EntrySelect::Kinds(vec![MESSAGE_CONTENT_TYPE.to_string()]),
            )
            .await
    }

    /// Remove the entries `content_ids` (with their chunks) of
    /// `session_id` in one batch, and invalidate its cached queries.
    async fn remove_stored_vectors(
        &self,
        session_id: Uuid,
        content_ids: &[String],
    ) -> Result<usize> {
        if content_ids.is_empty() {
            self.invalidate_cached(session_id);
            return Ok(0);
        }
        let removed = self
            .index()
            .delete(&session_id.to_string(), content_ids)
            .await;
        self.invalidate_cached(session_id);
        removed
    }
}
