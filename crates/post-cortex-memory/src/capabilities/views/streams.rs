// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The storage reads of the detail views: the stream stats and newest pages
//! of a scope's sessions, the turns behind search results, the embedding
//! queues; a few sessions at a time, in the order asked.

use std::collections::{BTreeSet, HashMap};

use futures::stream::{self, StreamExt, TryStreamExt};
use post_cortex_core::core::message::{self, Message, MessageCursor, MessageOrder, MessageStats};
use uuid::Uuid;

use crate::capabilities::messages::{MessageError, Messages};
use crate::capabilities::sessions::{SessionError, Sessions};
use crate::content_vectorizer::SemanticSearchResult;
use crate::graph_views::detail::Turns;

/// Sessions read from storage at once by one call.
const STORAGE_READS: usize = 8;

/// The stream stats of each session `ids`, in that order, in one read of
/// their summaries. (Owned ids: a borrowed slice would keep the future from
/// being `Send` for every lifetime, which a gRPC handler needs.)
pub async fn stats(messages: &Messages, ids: Vec<Uuid>) -> Result<Vec<MessageStats>, MessageError> {
    messages.stats_of(&ids).await
}

/// The newest `n` messages of each session `ids`, newest first, in that
/// order.
pub async fn newest(
    messages: &Messages,
    ids: Vec<Uuid>,
    n: usize,
) -> Result<Vec<Vec<Message>>, MessageError> {
    stream::iter(ids)
        .map(|id| async move {
            messages
                .page(id, MessageCursor::Before(u64::MAX), n, MessageOrder::Desc)
                .await
        })
        .buffered(STORAGE_READS)
        .try_collect()
        .await
}

/// The stored turns behind the message results of `results`.
pub async fn turns(
    messages: &Messages,
    results: &[SemanticSearchResult],
) -> Result<Turns, MessageError> {
    let wanted: BTreeSet<(Uuid, u64)> = results
        .iter()
        .filter_map(|r| {
            Some((
                r.source.session()?,
                message::seq_of_content_id(&r.content_id)?,
            ))
        })
        .collect();
    let pages: Vec<Vec<Message>> = stream::iter(wanted)
        .map(|(session, seq)| async move {
            messages
                .page(session, MessageCursor::From(seq), 1, MessageOrder::Asc)
                .await
                .map(|found| found.into_iter().filter(|m| m.seq == seq).collect())
        })
        .buffered(STORAGE_READS)
        .try_collect()
        .await?;
    let mut out: Turns = HashMap::new();
    for m in pages.into_iter().flatten() {
        out.insert((m.session_id, m.seq), m);
    }
    Ok(out)
}

/// Records of the sessions `ids` waiting for an embedding.
pub async fn pending(sessions: &Sessions, ids: Vec<Uuid>) -> Result<usize, SessionError> {
    stream::iter(ids)
        .map(|id| async move { sessions.pending_vectorization_count(id).await })
        .buffered(STORAGE_READS)
        .try_fold(0, |sum, n| async move { Ok(sum + n) })
        .await
}

/// The oldest `n` messages of each session `ids`, oldest first, in that
/// order.
pub async fn oldest(
    messages: &Messages,
    ids: Vec<Uuid>,
    n: usize,
) -> Result<Vec<Vec<Message>>, MessageError> {
    stream::iter(ids)
        .map(|id| async move {
            messages
                .page(id, MessageCursor::From(0), n, MessageOrder::Asc)
                .await
        })
        .buffered(STORAGE_READS)
        .try_collect()
        .await
}
