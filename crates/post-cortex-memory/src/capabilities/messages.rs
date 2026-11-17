// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Messages: the captured conversation turns of a session: append, page,
//! stats, delete, retention (ARCHITECTURE section 4).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use post_cortex_core::core::message::{
    self, Message, MessageCursor, MessageOrder, MessageStats, NewMessage,
};
use post_cortex_core::redact::{RedactionCounts, Redactor};
use post_cortex_core::services::{AppendMessagesRequest, AppendMessagesResponse};
use tracing::info;
use uuid::Uuid;

use crate::capabilities::derived::DerivedQueue;
use crate::memory_system::{Change, MemoryRuntime, MessagesChange, MessagesChangeKind};
use crate::pipeline::Embed;

/// Why a message operation failed.
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    /// The request is malformed.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// The session does not exist.
    #[error("Session {0} not found")]
    SessionNotFound(Uuid),
    /// Storage failed.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// Turns a page of a stream reads when the call names no limit.
pub const DEFAULT_PAGE: u32 = 100;

/// The most turns one page of a stream reads.
pub const MAX_PAGE: u32 = 1000;

/// A page of a session's stream, in order.
#[derive(Debug, Clone)]
pub struct StreamPage {
    /// The turns.
    pub messages: Vec<Message>,
    /// Where the next page starts; none on the last.
    pub next_from: Option<u64>,
}

/// The Messages capability service.
#[derive(Clone)]
pub struct Messages {
    rt: Arc<MemoryRuntime>,
    store: Arc<dyn post_cortex_ports::storage::MessageStore>,
    redactor: Arc<Redactor>,
    derived: DerivedQueue,
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> MessageError {
    MessageError::Storage(e.into())
}

impl Messages {
    pub(crate) fn new(
        rt: Arc<MemoryRuntime>,
        store: Arc<dyn post_cortex_ports::storage::MessageStore>,
        redactor: Arc<Redactor>,
        derived: DerivedQueue,
    ) -> Self {
        Self {
            rt,
            store,
            redactor,
            derived,
        }
    }

    /// Append captured turns to an existing session's stream: empty turns
    /// (no text and no blocks) are skipped, the rest redacted and stored; their embedding is queued
    /// on the derived-work pipeline (the capture path never embeds or loads
    /// the model). Idempotent on each turn's `turn_id`. The session is
    /// checked without loading it.
    ///
    /// # Errors
    /// `InvalidArgument` for a turn without `turn_id`, `SessionNotFound`,
    /// or storage failed.
    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %req.session_id, batch_size = req.messages.len()),
        name = "post_cortex.append_messages",
    )]
    pub async fn append(
        &self,
        mut req: AppendMessagesRequest,
    ) -> Result<AppendMessagesResponse, MessageError> {
        let submitted = req.messages.len();
        for (i, message) in req.messages.iter().enumerate() {
            if message.turn_id.trim().is_empty() {
                return Err(MessageError::InvalidArgument(format!(
                    "append_messages: messages[{i}].turn_id is empty"
                )));
            }
        }
        // A turn with blocks and no text (an agent's tool call) is kept.
        req.messages.retain(|m| !m.is_empty());
        let mut counts = RedactionCounts::new();
        for message in &mut req.messages {
            self.redactor.redact_message(message, &mut counts);
        }
        if !counts.is_empty() {
            tracing::info!(session_id = %req.session_id, ?counts, "append_messages: redacted sensitive values");
        }

        let stored = self.store(req.session_id, req.messages).await?;
        let appended: Vec<u64> = stored.iter().map(|m| m.seq).collect();
        self.publish(
            req.session_id,
            MessagesChangeKind::Appended,
            appended.clone(),
        );
        let queued = if self.rt.config.enable_embeddings && !appended.is_empty() {
            self.derived
                .submit(req.session_id, Embed::Messages(appended.clone()))
                .await;
            appended.len()
        } else {
            0
        };
        Ok(AppendMessagesResponse {
            skipped: submitted - stored.len(),
            appended,
            vectorized: 0,
            queued,
        })
    }

    /// Store turns in an existing session's stream (no redaction, no
    /// embedding); returns the turns stored. Turns already captured are
    /// skipped.
    pub(crate) async fn store(
        &self,
        session_id: Uuid,
        messages: Vec<NewMessage>,
    ) -> Result<Vec<Message>, MessageError> {
        // A stream never outlives (or precedes) its session.
        self.require_session(session_id).await?;
        let rt = &self.rt;
        // Exclusive per session: an append reads the next position before
        // writing.
        rt.persister
            .exclusive(session_id, self.store.append_messages(session_id, messages))
            .await
            .map_err(storage)
    }

    /// Count, first and last position (with their times) and the authors of
    /// a session's stream, read without loading it (empty for an unknown
    /// session).
    ///
    /// # Errors
    /// Storage failed.
    pub async fn stats(&self, session_id: Uuid) -> Result<MessageStats, MessageError> {
        self.rt
            .stores
            .messages
            .message_stats(session_id)
            .await
            .map_err(storage)
    }

    /// [`Self::stats`] of each of `session_ids`, in that order, in one read
    /// of the streams' summaries.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn stats_of(&self, session_ids: &[Uuid]) -> Result<Vec<MessageStats>, MessageError> {
        self.rt
            .stores
            .messages
            .message_stats_of(session_ids)
            .await
            .map_err(storage)
    }

    /// Up to `limit` messages of a session from `cursor`, read and returned
    /// in `order`; positions retention removed are skipped.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn page(
        &self,
        session_id: Uuid,
        cursor: MessageCursor,
        limit: usize,
        order: MessageOrder,
    ) -> Result<Vec<Message>, MessageError> {
        self.rt
            .stores
            .messages
            .load_messages_page(session_id, cursor, limit, order)
            .await
            .map_err(storage)
    }

    /// Up to `limit` turns of a session from position `from`, in order
    /// (none: `DEFAULT_PAGE`), and where the next page starts.
    ///
    /// # Errors
    /// `InvalidArgument` for a limit of 0 or over `MAX_PAGE`; storage
    /// failed.
    pub async fn stream(
        &self,
        session_id: Uuid,
        from: u64,
        limit: Option<u32>,
    ) -> Result<StreamPage, MessageError> {
        let limit = limit.unwrap_or(DEFAULT_PAGE);
        if limit == 0 || limit > MAX_PAGE {
            return Err(MessageError::InvalidArgument(format!(
                "a page takes a limit from 1 to {MAX_PAGE}"
            )));
        }
        let messages = self
            .page(
                session_id,
                MessageCursor::From(from),
                limit as usize,
                MessageOrder::Asc,
            )
            .await?;
        let next_from = (messages.len() == limit as usize)
            .then(|| messages.last().map(|m| m.seq + 1))
            .flatten();
        Ok(StreamPage {
            messages,
            next_from,
        })
    }

    /// Delete one turn and its vectors; returns whether it was stored. Its
    /// position is not reused; records whose `derived_from` names it keep
    /// the id as provenance.
    ///
    /// # Errors
    /// The session does not exist or storage failed.
    pub async fn delete(&self, session_id: Uuid, seq: u64) -> Result<bool, MessageError> {
        self.require_session(session_id).await?;
        let rt = &self.rt;
        let deleted = rt
            .persister
            .exclusive(session_id, self.store.delete_message(session_id, seq))
            .await
            .map_err(storage)?;
        if deleted {
            rt.drop_content_vectors(session_id, &[message::content_id(seq)])
                .await;
            self.publish(session_id, MessagesChangeKind::Deleted, vec![seq]);
        }
        Ok(deleted)
    }

    /// Delete a session's whole stream and its vectors; returns how many
    /// turns were deleted.
    ///
    /// # Errors
    /// The session does not exist or storage failed.
    pub async fn delete_all(&self, session_id: Uuid) -> Result<usize, MessageError> {
        self.require_session(session_id).await?;
        let rt = &self.rt;
        let deleted = rt
            .persister
            .exclusive(session_id, self.store.delete_session_messages(session_id))
            .await
            .map_err(storage)?;
        let content_ids: Vec<String> = deleted
            .iter()
            .map(|&seq| message::content_id(seq))
            .collect();
        rt.drop_content_vectors(session_id, &content_ids).await;
        let count = deleted.len();
        self.publish(session_id, MessagesChangeKind::Deleted, deleted);
        if count > 0 {
            info!("Deleted {count} messages of session {session_id}");
        }
        Ok(count)
    }

    /// Delete every message timestamped before `cutoff` (retention), with
    /// its vectors; returns how many were deleted.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn expire(&self, cutoff: DateTime<Utc>) -> Result<usize, MessageError> {
        let rt = &self.rt;
        let deleted = rt
            .stores
            .messages
            .delete_messages_before(cutoff)
            .await
            .map_err(storage)?;
        let mut by_session: HashMap<Uuid, Vec<u64>> = HashMap::new();
        for (session_id, seq) in &deleted {
            by_session.entry(*session_id).or_default().push(*seq);
        }
        for (session_id, seqs) in by_session {
            let content_ids: Vec<String> =
                seqs.iter().map(|&seq| message::content_id(seq)).collect();
            rt.drop_content_vectors(session_id, &content_ids).await;
            self.publish(session_id, MessagesChangeKind::Deleted, seqs);
        }
        if !deleted.is_empty() {
            info!("Expired {} messages older than {}", deleted.len(), cutoff);
        }
        Ok(deleted.len())
    }

    /// Publish that `seqs` of `session_id`'s stream were appended or deleted
    /// (nothing when there are none).
    fn publish(&self, session_id: Uuid, kind: MessagesChangeKind, seqs: Vec<u64>) {
        if !seqs.is_empty() {
            self.rt.changes.publish(Change::Messages(MessagesChange {
                session_id,
                kind,
                seqs,
                at: Utc::now(),
            }));
        }
    }

    /// The session exists: cached, else stored (checked without loading it).
    async fn require_session(&self, session_id: Uuid) -> Result<(), MessageError> {
        if self.rt.session_manager.cached(session_id).is_some()
            || self
                .rt
                .stores
                .sessions
                .session_exists(session_id)
                .await
                .map_err(storage)?
        {
            Ok(())
        } else {
            Err(MessageError::SessionNotFound(session_id))
        }
    }
}
