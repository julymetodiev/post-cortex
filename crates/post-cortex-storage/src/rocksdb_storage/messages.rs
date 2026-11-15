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

//! The raw message stream and the [`MessageStore`] trait implementation.
//!
//! Key families:
//! - `message:{session_id}:{seq}` -> marked bincode [`Message`] (see
//!   [`crate::rocksdb_storage::rows`]); `seq` is written
//!   as 20 zero-padded digits, so key order is stream order.
//! - `message_turn:{session_id}:{turn_id}` -> `seq` (8 bytes, big endian),
//!   the idempotency index.
//! - `message_next:{session_id}` -> the next `seq` (8 bytes, big endian).
//!   Kept rather than derived from the last row, so positions are never
//!   reused after retention empties a stream.
//! - `message_count:{session_id}` -> messages stored (8 bytes, big endian),
//!   so stats need no scan. Streams written before it existed get it from
//!   one scan, the first time the session's lock is taken for them.
//! - `message_authors:{session_id}` -> the authors of the stored messages,
//!   each once, sorted (a JSON array): an append adds its authors, a delete
//!   recounts them from the rows left. Streams written before it existed
//!   get it from one scan, as the count.
//!
//! An append, and retention's delete of one session's rows, is one
//! `WriteBatch` under the session's lock.

use crate::port_error::PortResult;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rocksdb::{Direction, IteratorMode, ReadOptions};
use std::collections::BTreeMap;
use uuid::Uuid;

use post_cortex_core::core::message::{
    Message, MessageCursor, MessageOrder, MessageStats, NewMessage,
};
use post_cortex_ports::storage::MessageStore;

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, scan_prefix};

const MESSAGE_PREFIX: &str = "message:";

pub fn message_key(session_id: Uuid, seq: u64) -> String {
    format!("message:{}:{:020}", session_id, seq)
}

pub fn message_turn_key(session_id: Uuid, turn_id: &str) -> String {
    format!("message_turn:{}:{}", session_id, turn_id)
}

pub fn message_next_key(session_id: Uuid) -> String {
    format!("message_next:{}", session_id)
}

pub fn message_count_key(session_id: Uuid) -> String {
    format!("message_count:{}", session_id)
}

pub fn message_authors_key(session_id: Uuid) -> String {
    format!("message_authors:{}", session_id)
}

mod stream;

use stream::{append, decode_message, delete_seqs, expire, page, restore, stats, stream_seqs};

#[async_trait]
impl MessageStore for RealRocksDBStorage {
    async fn delete_message(&self, session_id: Uuid, seq: u64) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(session_id);
                    Ok(!delete_seqs(&db, session_id, &[seq])?.is_empty())
                })
                .await
            })
            .await
    }

    async fn delete_session_messages(&self, session_id: Uuid) -> PortResult<Vec<u64>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(session_id);
                    let seqs = stream_seqs(&db, session_id)?;
                    delete_seqs(&db, session_id, &seqs)
                })
                .await
            })
            .await
    }

    async fn append_messages(
        &self,
        session_id: Uuid,
        messages: Vec<NewMessage>,
    ) -> PortResult<Vec<Message>> {
        self.breaker
            .guard(async {
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
            })
            .await
    }

    async fn restore_messages(
        &self,
        session_id: Uuid,
        messages: Vec<Message>,
    ) -> PortResult<Vec<(u64, Message)>> {
        self.breaker
            .guard(async {
                if messages.is_empty() {
                    return Ok(Vec::new());
                }
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(session_id);
                    restore(&db, session_id, messages)
                })
                .await
            })
            .await
    }

    async fn load_messages(
        &self,
        session_id: Uuid,
        seqs: std::ops::Range<u64>,
    ) -> PortResult<Vec<Message>> {
        self.breaker
            .guard(async {
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
                        let (key, value) = item?;
                        messages.push(decode_message(&key, &value)?);
                    }
                    Ok(messages)
                })
                .await
            })
            .await
    }

    async fn message_stats(&self, session_id: Uuid) -> PortResult<MessageStats> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(session_id);
                    stats(&db, session_id)
                })
                .await
            })
            .await
    }

    async fn message_stats_of(&self, session_ids: &[Uuid]) -> PortResult<Vec<MessageStats>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                let ids = session_ids.to_vec();
                blocking(move || {
                    ids.into_iter()
                        .map(|session_id| {
                            let _guard = locks.lock(session_id);
                            stats(&db, session_id)
                        })
                        .collect()
                })
                .await
            })
            .await
    }

    async fn load_messages_page(
        &self,
        session_id: Uuid,
        cursor: MessageCursor,
        limit: usize,
        order: MessageOrder,
    ) -> PortResult<Vec<Message>> {
        self.breaker
            .guard(async {
                if limit == 0 {
                    return Ok(Vec::new());
                }
                let db = self.db.clone();
                blocking(move || page(&db, session_id, cursor, limit, order)).await
            })
            .await
    }

    async fn delete_messages_before(&self, cutoff: DateTime<Utc>) -> PortResult<Vec<(Uuid, u64)>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    // Found without locks, deleted per session under its lock.
                    let mut expired: BTreeMap<Uuid, Vec<u64>> = BTreeMap::new();
                    for item in scan_prefix(&db, MESSAGE_PREFIX.as_bytes()) {
                        let (key, value) = item?;
                        let message = match decode_message(&key, &value) {
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
                            expired
                                .entry(message.session_id)
                                .or_default()
                                .push(message.seq);
                        }
                    }
                    let mut deleted = Vec::new();
                    for (session_id, seqs) in expired {
                        let _guard = locks.lock(session_id);
                        deleted.extend(
                            expire(&db, session_id, &seqs, cutoff)?
                                .into_iter()
                                .map(|seq| (session_id, seq)),
                        );
                    }
                    Ok(deleted)
                })
                .await
            })
            .await
    }
}

#[cfg(test)]
mod tests;
