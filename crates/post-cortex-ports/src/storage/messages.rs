// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The message port: the captured conversation stream.

use crate::error::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use post_cortex_core::core::message::{
    Message, MessageCursor, MessageOrder, MessageStats, NewMessage,
};
use uuid::Uuid;

/// Storage for the raw message stream (see
/// [`post_cortex_core::core::message`]).
///
/// Appends to one session must not run concurrently: the next position is
/// read and then written. The memory layer serialises them per session.
#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Append `messages` to the stream of `session_id`, in the given order,
    /// at the positions after the last stored one. Messages whose `turn_id`
    /// is already stored for the session, or repeats an earlier one in the
    /// batch, are skipped. Returns the messages appended.
    async fn append_messages(
        &self,
        session_id: Uuid,
        messages: Vec<NewMessage>,
    ) -> Result<Vec<Message>>;

    /// The stored messages of `session_id` with `seq` in `seqs`, by `seq`.
    async fn load_messages(
        &self,
        session_id: Uuid,
        seqs: std::ops::Range<u64>,
    ) -> Result<Vec<Message>>;

    /// Count, first and last position (with their timestamps) and the
    /// authors of the stored stream of `session_id`, without loading it.
    /// After retention the first position is that of the oldest message
    /// left.
    async fn message_stats(&self, session_id: Uuid) -> Result<MessageStats>;

    /// [`Self::message_stats`] of each of `session_ids`, in that order, in
    /// one read of the streams' summaries (a session without messages has
    /// empty stats). Every write of a stream keeps its summary in the
    /// write's own transaction; a stream written before summaries existed
    /// gets one from its rows the first time it is read.
    async fn message_stats_of(&self, session_ids: &[Uuid]) -> Result<Vec<MessageStats>>;

    /// Up to `limit` stored messages of `session_id` from the positions
    /// `cursor` names, read in `order` and returned in that order. Gaps
    /// left by retention are skipped, so a page is full while the stream
    /// has messages on that side. The next page starts at
    /// `From(last.seq + 1)` ascending, `Before(last.seq)` descending.
    /// E.g. `(Before(u64::MAX), Desc)` is the newest page.
    async fn load_messages_page(
        &self,
        session_id: Uuid,
        cursor: MessageCursor,
        limit: usize,
        order: MessageOrder,
    ) -> Result<Vec<Message>>;

    /// Delete every message, in all sessions, whose timestamp is before
    /// `cutoff` (retention). Returns `(session_id, seq)` of each deleted
    /// message so the caller can drop its vectors.
    async fn delete_messages_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<(Uuid, u64)>>;

    /// Delete the message of `session_id` at `seq`; returns whether it was
    /// stored. Positions are never reused: the next append still gets the
    /// position after the last one ever stored.
    async fn delete_message(&self, session_id: Uuid, seq: u64) -> Result<bool>;

    /// Delete every message of `session_id`'s stream; returns the positions
    /// deleted. As with [`Self::delete_message`], positions are not reused.
    async fn delete_session_messages(&self, session_id: Uuid) -> Result<Vec<u64>>;

    /// Write archived `messages` into the stream of `session_id` (an
    /// import). A message whose `turn_id` is stored already, or repeats an
    /// earlier one, is skipped; the others, by `seq`, keep their `seq` when
    /// it is at or past the stream's next position and go to the end
    /// otherwise, so a position is never reused. The next position and the
    /// count follow. Returns `(archived seq, message as stored)` of each
    /// message written.
    async fn restore_messages(
        &self,
        session_id: Uuid,
        messages: Vec<Message>,
    ) -> Result<Vec<(u64, Message)>>;
}
