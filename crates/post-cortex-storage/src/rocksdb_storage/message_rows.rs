// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The rows of a message stream (`message:{session}:{seq}`). Version 2
//! (W18) adds the turn a message follows and its blocks (JSON, see
//! `message_blocks`); a version 1 row reads with neither. Rows are
//! written as version 2.

use chrono::{DateTime, Utc};
use post_cortex_core::core::message::{Message, MessageBlock, MessageRole};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::port_error::Result;
use crate::rocksdb_storage::rows::{MESSAGE_V1, MESSAGE_V2, decode_marked, encode_marked};

/// A message row of version 1: a captured turn, its fields in the order
/// bincode wrote them.
#[derive(Serialize, Deserialize)]
pub struct MessageRowV1 {
    session_id: Uuid,
    seq: u64,
    turn_id: String,
    role: MessageRole,
    author: Option<String>,
    source_session: Option<String>,
    timestamp: DateTime<Utc>,
    text: String,
    tools: Vec<String>,
    files: Vec<String>,
}

/// A message row of version 2: version 1, then its parent and blocks.
#[derive(Serialize, Deserialize)]
struct MessageRowV2 {
    first: MessageRowV1,
    parent: Option<String>,
    blocks: Option<String>,
}

impl MessageRowV1 {
    /// The version 1 fields of `message`.
    pub fn of(message: &Message) -> Self {
        Self {
            session_id: message.session_id,
            seq: message.seq,
            turn_id: message.turn_id.clone(),
            role: message.role,
            author: message.author.clone(),
            source_session: message.source_session.clone(),
            timestamp: message.timestamp,
            text: message.text.clone(),
            tools: message.tools.clone(),
            files: message.files.clone(),
        }
    }

    fn into_message(self, parent: Option<String>, blocks: Vec<MessageBlock>) -> Message {
        Message {
            session_id: self.session_id,
            seq: self.seq,
            turn_id: self.turn_id,
            role: self.role,
            author: self.author,
            source_session: self.source_session,
            timestamp: self.timestamp,
            text: self.text,
            tools: self.tools,
            files: self.files,
            parent,
            blocks,
        }
    }
}

/// `message` as a version 2 row.
pub fn encode(message: &Message) -> Result<Vec<u8>> {
    let row = MessageRowV2 {
        blocks: crate::message_blocks::encode(&message.turn_id, &message.blocks)?,
        parent: message.parent.clone(),
        first: MessageRowV1::of(message),
    };
    encode_marked(MESSAGE_V2, &row)
}

/// The message of a version 1 or 2 row at `key`.
pub fn decode(key: &[u8], row: &[u8]) -> Result<Message> {
    let key = String::from_utf8_lossy(key);
    if row.starts_with(MESSAGE_V1) {
        let v1: MessageRowV1 = decode_marked("message", &key, MESSAGE_V1, row)?;
        return Ok(v1.into_message(None, Vec::new()));
    }
    let v2: MessageRowV2 = decode_marked("message", &key, MESSAGE_V2, row)?;
    let blocks = crate::message_blocks::decode(&v2.first.turn_id, v2.blocks.as_deref())?;
    Ok(v2.first.into_message(v2.parent, blocks))
}
