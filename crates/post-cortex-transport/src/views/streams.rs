// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The capability's message streams as `pcx.views.v1` messages: a turn, the
//! streams of a scope, a page of one, and a watch's events. Nothing is
//! computed here.

use post_cortex_core::core::conversation::ConversationState;
use post_cortex_memory::RecordEventKind;
use post_cortex_memory::capabilities::{ActivityEvent, ActivityKind};
use post_cortex_memory::graph_views::detail::{ConversationPage, Conversations, TurnDetail};
use post_cortex_proto::views as pb;
use tonic::Status;

use crate::views::response::{ids, n, record, timestamp};

/// A turn of a stream.
///
/// # Errors
/// Its blocks do not map.
pub fn message(t: TurnDetail) -> Result<pb::Message, Status> {
    let m = t.message;
    Ok(pb::Message {
        session_id: m.session_id.to_string(),
        seq: m.seq,
        content_id: post_cortex_core::core::message::content_id(m.seq),
        turn_id: m.turn_id,
        role: m.role.as_str().to_string(),
        author: m.author,
        timestamp: Some(timestamp(m.timestamp)),
        text: m.text,
        tools: m.tools,
        files: m.files,
        records: ids(&t.records),
        source_session: m.source_session,
        blocks: m
            .blocks
            .into_iter()
            .map(crate::blocks::to_wire)
            .collect::<Result<_, Status>>()?,
    })
}

/// The streams of a scope.
pub fn conversations(list: Conversations) -> pb::Conversations {
    pb::Conversations {
        total: n(list.total),
        next_cursor: list.next.map(|c| c.to_string()).unwrap_or_default(),
        conversations: list
            .conversations
            .into_iter()
            .map(|c| pb::ConversationSummary {
                session_id: c.session_id.to_string(),
                name: c.name,
                messages: c.messages,
                first_at: c.first_at.map(timestamp),
                last_at: c.last_at.map(timestamp),
                authors: c.authors,
                derived_records: n(c.derived_records),
                asker: c.asker,
                first_question: c.first_question,
                state: match c.state {
                    None => pb::ConversationState::Unspecified,
                    Some(ConversationState::Idle) => pb::ConversationState::ConversationIdle,
                    Some(ConversationState::Running) => pb::ConversationState::ConversationRunning,
                    Some(ConversationState::Failed) => pb::ConversationState::ConversationFailed,
                } as i32,
            })
            .collect(),
    }
}

/// A page of a stream.
///
/// # Errors
/// A turn whose blocks do not map.
pub fn conversation(p: ConversationPage) -> Result<pb::Conversation, Status> {
    Ok(pb::Conversation {
        session_id: p.session_id.to_string(),
        name: p.name,
        messages: p.turns.into_iter().map(message).collect::<Result<_, _>>()?,
        next_cursor: p.next,
        prev_cursor: p.prev,
        total: p.total,
    })
}

/// A record change or a message of a watch.
///
/// # Errors
/// A message whose blocks do not map.
pub fn activity(e: ActivityEvent) -> Result<pb::ActivityEvent, Status> {
    Ok(pb::ActivityEvent {
        record: e.record.map(record),
        kind: match e.kind {
            ActivityKind::Record(RecordEventKind::Written) => pb::ActivityKind::ActivityWritten,
            ActivityKind::Record(RecordEventKind::Forgotten) => pb::ActivityKind::ActivityForgotten,
            ActivityKind::Record(RecordEventKind::Removed) => pb::ActivityKind::ActivityRemoved,
            ActivityKind::Message => pb::ActivityKind::ActivityMessage,
        } as i32,
        record_id: e.record_id.map(|id| id.to_string()).unwrap_or_default(),
        session_id: e.session_id.map(|s| s.to_string()).unwrap_or_default(),
        at: Some(timestamp(e.at)),
        message: e.message.map(message).transpose()?,
    })
}
