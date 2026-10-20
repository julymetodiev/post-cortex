// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Raw conversation turns (ROADMAP Phase 2 item 1).
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

impl MessageRole {
    /// Lowercase name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    /// Parse the lowercase name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }
}

/// A turn as a client submits it, before the store gives it a position.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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
        }
    }
}

/// A stored turn. Fields as in [`NewMessage`], plus its place in the stream.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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
