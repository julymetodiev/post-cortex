// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! The raw message stream and the [`MessageStorage`] trait implementation.
//!
//! Key families:
//! - `message:{session_id}:{seq}` -> bincode [`Message`]; `seq` is written
//!   as 20 zero-padded digits, so key order is stream order.
//! - `message_turn:{session_id}:{turn_id}` -> `seq` (8 bytes, big endian),
//!   the idempotency index.
//! - `message_next:{session_id}` -> the next `seq` (8 bytes, big endian).
//!   Kept rather than derived from the last row, so positions are never
//!   reused after retention empties a stream.
//!
//! An append is one `WriteBatch` under the session's lock.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rocksdb::{DB, Direction, IteratorMode, ReadOptions, WriteBatch};
use std::collections::HashSet;
use uuid::Uuid;

use crate::traits::MessageStorage;
use post_cortex_core::core::message::{Message, NewMessage};

use super::RealRocksDBStorage;
use super::core::{blocking, scan_prefix};

const MESSAGE_PREFIX: &str = "message:";

pub(super) fn message_key(session_id: Uuid, seq: u64) -> String {
    format!("message:{}:{:020}", session_id, seq)
}

pub(super) fn message_turn_key(session_id: Uuid, turn_id: &str) -> String {
    format!("message_turn:{}:{}", session_id, turn_id)
}

pub(super) fn message_next_key(session_id: Uuid) -> String {
    format!("message_next:{}", session_id)
}

fn encode_message(message: &Message) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(message, bincode::config::standard())
        .map_err(|e| anyhow::anyhow!("Failed to serialize message: {}", e))
}

fn decode_message(bytes: &[u8]) -> Result<Message> {
    bincode::serde::decode_from_slice::<Message, _>(bytes, bincode::config::standard())
        .map(|(m, _)| m)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize message: {}", e))
}

fn decode_seq(bytes: &[u8]) -> Result<u64> {
    let raw: [u8; 8] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Malformed message seq ({} bytes)", bytes.len()))?;
    Ok(u64::from_be_bytes(raw))
}

/// Append `messages` to `session_id`'s stream. The caller holds the
/// session's lock.
fn append(db: &DB, session_id: Uuid, messages: Vec<NewMessage>) -> Result<Vec<Message>> {
    let next_key = message_next_key(session_id);
    let mut next = match db.get(next_key.as_bytes())? {
        Some(bytes) => decode_seq(&bytes)?,
        None => 0,
    };
    let mut seen = HashSet::new();
    let mut batch = WriteBatch::default();
    let mut appended = Vec::new();
    for new in messages {
        let turn_key = message_turn_key(session_id, &new.turn_id);
        if !seen.insert(new.turn_id.clone()) || db.get(turn_key.as_bytes())?.is_some() {
            continue;
        }
        let message = new.into_message(session_id, next);
        batch.put(
            message_key(session_id, next).as_bytes(),
            encode_message(&message)?,
        );
        batch.put(turn_key.as_bytes(), next.to_be_bytes());
        appended.push(message);
        next += 1;
    }
    if !appended.is_empty() {
        batch.put(next_key.as_bytes(), next.to_be_bytes());
        db.write(batch)?;
    }
    Ok(appended)
}

#[async_trait]
impl MessageStorage for RealRocksDBStorage {
    async fn append_messages(
        &self,
        session_id: Uuid,
        messages: Vec<NewMessage>,
    ) -> Result<Vec<Message>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let locks = self.session_locks.clone();
        blocking(move || {
            let _guard = locks.lock(session_id);
            append(&db, session_id, messages)
        })
        .await
    }

    async fn load_messages(
        &self,
        session_id: Uuid,
        seqs: std::ops::Range<u64>,
    ) -> Result<Vec<Message>> {
        if seqs.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        blocking(move || {
            let start = message_key(session_id, seqs.start);
            let mut opts = ReadOptions::default();
            opts.set_total_order_seek(true);
            opts.set_iterate_upper_bound(message_key(session_id, seqs.end).into_bytes());
            let mut messages = Vec::new();
            for item in db.iterator_opt(
                IteratorMode::From(start.as_bytes(), Direction::Forward),
                opts,
            ) {
                let (_, value) = item?;
                messages.push(decode_message(&value)?);
            }
            Ok(messages)
        })
        .await
    }

    async fn delete_messages_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<(Uuid, u64)>> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            let mut deleted = Vec::new();
            for item in scan_prefix(&db, MESSAGE_PREFIX.as_bytes()) {
                let (key, value) = item?;
                let message = match decode_message(&value) {
                    Ok(message) => message,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping unreadable message {}: {:#}",
                            String::from_utf8_lossy(&key),
                            e
                        );
                        continue;
                    }
                };
                if message.timestamp < cutoff {
                    batch.delete(&key);
                    batch.delete(message_turn_key(message.session_id, &message.turn_id).as_bytes());
                    deleted.push((message.session_id, message.seq));
                }
            }
            if !deleted.is_empty() {
                db.write(batch)?;
            }
            Ok(deleted)
        })
        .await
    }
}
