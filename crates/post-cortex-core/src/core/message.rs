// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Raw conversation turns.
//!
//! Client hooks capture every turn of a conversation into a per-session,
//! append-only stream. Messages are not [`ContextUpdate`]s: they never enter
//! `ActiveSession`, so they do not reach the structured summary, the entity
//! graph or the importance scores, and loading a session does not load them.
//! Each message has a position `seq` in its session's stream (0, 1, 2, ...),
//! which orders the stream and drives neighbour expansion in search.
//!
//! [`ContextUpdate`]: crate::core::context_update::ContextUpdate

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Prefix of a message's content id in the vector index: `message:<seq>`
/// (chunks of a long message add `#<n>`, like updates).
pub const CONTENT_ID_PREFIX: &str = "message:";

/// Content id of the message at position `seq`.
pub fn content_id(seq: u64) -> String {
    format!("{CONTENT_ID_PREFIX}{seq}")
}

/// Position of the message a content id (or one of its chunks) names.
pub fn seq_of_content_id(content_id: &str) -> Option<u64> {
    let rest = content_id.strip_prefix(CONTENT_ID_PREFIX)?;
    rest.split('#').next()?.parse().ok()
}

/// Who produced a turn.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// The person talking to the agent.
    User,
    /// The agent's reply.
    Assistant,
}

crate::core::wire::wire_names!(MessageRole, "message role", {
    User => "user",
    Assistant => "assistant",
});

/// One piece of a turn as an agent keeps it whole (the harness's agent,
/// ARCHITECTURE section 7): what resumes a session; `text` stays what
/// search and views read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageBlock {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
    /// The model's reasoning.
    Thinking {
        /// The reasoning.
        text: String,
        /// The provider's signature of it, when it gave one.
        signature: Option<String>,
    },
    /// A tool call.
    ToolUse {
        /// The call's id, which its result names.
        id: String,
        /// The tool.
        name: String,
        /// Its input (a JSON object).
        input: serde_json::Value,
    },
    /// What the turns before it said, in short (a compaction): an agent
    /// resumes from the newest one.
    Summary {
        /// The summary.
        text: String,
    },
    /// A tool's result.
    ToolResult {
        /// The call it answers.
        tool_use_id: String,
        /// Its text.
        content: String,
        /// Whether the tool failed.
        is_error: bool,
    },
    /// How the turn it closes ended (W25): kept in the transcript, never
    /// sent to a model.
    End {
        /// Why it ended (`end_turn`, `cancelled`, `failed`, ...).
        stop: String,
        /// What happened, for a turn blocked, failed or cancelled.
        detail: String,
    },
}

/// A turn as a client submits it, before the store gives it a position.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NewMessage {
    /// The client's id for the turn (e.g. the Claude Code transcript entry
    /// uuid). Appends are idempotent on it within a session.
    pub turn_id: String,
    /// Who produced the turn.
    pub role: MessageRole,
    /// Agent name, user or teammate who wrote it, when known.
    pub author: Option<String>,
    /// The client's own conversation id (e.g. the Claude Code session id),
    /// which is not the post-cortex session.
    pub source_session: Option<String>,
    /// When the turn happened.
    pub timestamp: DateTime<Utc>,
    /// The user prompt or the assistant's final text, whole.
    pub text: String,
    /// Names of the tools the assistant used in the turn.
    pub tools: Vec<String>,
    /// Paths of the files the turn read or changed.
    pub files: Vec<String>,
    /// The `turn_id` of the turn it follows, for an agent's forks; none:
    /// it follows the one before it in the stream. Left out of the JSON
    /// when none (archives before 1.6.0 have none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The turn whole, as an agent keeps it; empty for a captured turn,
    /// and then left out of the JSON.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<MessageBlock>,
}

impl NewMessage {
    /// This turn stored at position `seq` of session `session_id`.
    pub fn into_message(self, session_id: Uuid, seq: u64) -> Message {
        Message {
            session_id,
            seq,
            turn_id: self.turn_id,
            role: self.role,
            author: self.author,
            source_session: self.source_session,
            timestamp: self.timestamp,
            text: self.text,
            tools: self.tools,
            files: self.files,
            parent: self.parent,
            blocks: self.blocks,
        }
    }

    /// Whether it carries nothing to keep: no text and no blocks.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.blocks.is_empty()
    }
}

/// A stored turn. Fields as in [`NewMessage`], plus its place in the stream.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message {
    /// Session whose stream holds the message.
    pub session_id: Uuid,
    /// Position in the session's stream, from 0, without gaps at append
    /// time (retention later removes the oldest).
    pub seq: u64,
    /// See [`NewMessage::turn_id`].
    pub turn_id: String,
    /// See [`NewMessage::role`].
    pub role: MessageRole,
    /// See [`NewMessage::author`].
    pub author: Option<String>,
    /// See [`NewMessage::source_session`].
    pub source_session: Option<String>,
    /// See [`NewMessage::timestamp`].
    pub timestamp: DateTime<Utc>,
    /// See [`NewMessage::text`].
    pub text: String,
    /// See [`NewMessage::tools`].
    pub tools: Vec<String>,
    /// See [`NewMessage::files`].
    pub files: Vec<String>,
    /// See [`NewMessage::parent`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// See [`NewMessage::blocks`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<MessageBlock>,
}

/// What a session's stored message stream holds, read without loading it.
///
/// Positions are contiguous except where retention removed turns, so
/// `count` can be less than `last_seq - first_seq + 1`. An empty stream
/// (none captured, or all expired) has `count` 0, no positions or times and
/// no authors.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageStats {
    /// Messages stored.
    pub count: u64,
    /// Position of the oldest stored message.
    pub first_seq: Option<u64>,
    /// Position of the newest stored message.
    pub last_seq: Option<u64>,
    /// Timestamp of the message at `first_seq`.
    pub first_at: Option<DateTime<Utc>>,
    /// Timestamp of the message at `last_seq`.
    pub last_at: Option<DateTime<Utc>>,
    /// The authors of the stored messages, each once, sorted (messages
    /// without an author add none).
    pub authors: Vec<String>,
}

/// Which positions a page of a message stream is taken from.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageCursor {
    /// Positions at or after this one.
    From(u64),
    /// Positions before this one.
    Before(u64),
}

/// Order a page of a message stream is read and returned in.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageOrder {
    /// Oldest first.
    #[default]
    Asc,
    /// Newest first.
    Desc,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_ids_name_the_position() {
        assert_eq!(content_id(7), "message:7");
        assert_eq!(seq_of_content_id("message:7"), Some(7));
        assert_eq!(seq_of_content_id("message:7#2"), Some(7));
        assert_eq!(seq_of_content_id("entity:message"), None);
        assert_eq!(seq_of_content_id("message:x"), None);
    }
}
