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
//! Rows live in the `message` table keyed `{session_id}_{seq}`, unique on
//! `(session_id, seq)` and on `(session_id, turn_id)`. The next position of
//! each stream is kept in `message_next` (keyed by session id), so
//! positions are never reused after retention empties a stream. An append
//! writes its rows and the new position in one transaction.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::traits::MessageStorage;
use post_cortex_core::core::message::{Message, MessageRole, NewMessage};

use super::SurrealDBStorage;

/// Row of the `message` table.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct MessageRecord {
    session_id: String,
    seq: i64,
    turn_id: String,
    role: String,
    author: Option<String>,
    source_session: Option<String>,
    timestamp: String,
    text: String,
    tools: Vec<String>,
    files: Vec<String>,
}

impl MessageRecord {
    fn from_message(message: &Message) -> Self {
        Self {
            session_id: message.session_id.to_string(),
            seq: message.seq as i64,
            turn_id: message.turn_id.clone(),
            role: message.role.as_str().to_string(),
            author: message.author.clone(),
            source_session: message.source_session.clone(),
            timestamp: message.timestamp.to_rfc3339(),
            text: message.text.clone(),
            tools: message.tools.clone(),
            files: message.files.clone(),
        }
    }

    fn into_message(self) -> Result<Message> {
        Ok(Message {
            session_id: Uuid::parse_str(&self.session_id)?,
            seq: u64::try_from(self.seq)?,
            role: MessageRole::parse(&self.role)
                .ok_or_else(|| anyhow::anyhow!("Unknown message role '{}'", self.role))?,
            turn_id: self.turn_id,
            author: self.author,
            source_session: self.source_session,
            timestamp: SurrealDBStorage::parse_datetime(&self.timestamp),
            text: self.text,
            tools: self.tools,
            files: self.files,
        })
    }
}

/// `message` row for the batched insert.
#[derive(Serialize, SurrealValue)]
struct KeyedMessage {
    /// `{session_id}_{seq}`.
    key: String,
    record: MessageRecord,
}

/// What an append needs to know about the stored stream.
#[derive(Deserialize, SurrealValue)]
struct StreamState {
    next: Option<i64>,
    turns: Vec<String>,
}

/// Position of a deleted message.
#[derive(Deserialize, SurrealValue)]
struct DeletedMessage {
    session_id: String,
    seq: i64,
}

#[async_trait]
impl MessageStorage for SurrealDBStorage {
    async fn append_messages(
        &self,
        session_id: Uuid,
        messages: Vec<NewMessage>,
    ) -> Result<Vec<Message>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let sid = session_id.to_string();
        let turns: Vec<String> = messages.iter().map(|m| m.turn_id.clone()).collect();
        let mut response = self
            .db
            .query(
                "RETURN { \
                     next: (SELECT VALUE next FROM ONLY type::record('message_next', $sid)), \
                     turns: (SELECT VALUE turn_id FROM message \
                         WHERE session_id = $sid AND turn_id IN $turns) \
                 };",
            )
            .bind(("sid", sid.clone()))
            .bind(("turns", turns))
            .await?
            .check()?;
        let state: Option<StreamState> = response.take(0)?;
        let state = state.ok_or_else(|| anyhow::anyhow!("No message stream state returned"))?;

        let mut next = u64::try_from(state.next.unwrap_or(0))?;
        let mut seen: HashSet<String> = state.turns.into_iter().collect();
        let mut appended = Vec::new();
        for new in messages {
            if !seen.insert(new.turn_id.clone()) {
                continue;
            }
            appended.push(new.into_message(session_id, next));
            next += 1;
        }
        if appended.is_empty() {
            return Ok(appended);
        }

        let rows: Vec<KeyedMessage> = appended
            .iter()
            .map(|m| KeyedMessage {
                key: format!("{}_{}", sid, m.seq),
                record: MessageRecord::from_message(m),
            })
            .collect();
        self.db
            .query(
                "BEGIN TRANSACTION; \
                 FOR $m IN $rows { CREATE type::record('message', $m.key) CONTENT $m.record; }; \
                 UPSERT type::record('message_next', $sid) SET next = $next; \
                 COMMIT TRANSACTION;",
            )
            .bind(("rows", rows))
            .bind(("sid", sid))
            .bind(("next", next as i64))
            .await?
            .check()?;
        Ok(appended)
    }

    async fn load_messages(
        &self,
        session_id: Uuid,
        seqs: std::ops::Range<u64>,
    ) -> Result<Vec<Message>> {
        if seqs.is_empty() {
            return Ok(Vec::new());
        }
        let mut response = self
            .db
            .query(
                "SELECT * OMIT id FROM message \
                 WHERE session_id = $sid AND seq >= $start AND seq < $end ORDER BY seq",
            )
            .bind(("sid", session_id.to_string()))
            .bind(("start", i64::try_from(seqs.start)?))
            .bind(("end", i64::try_from(seqs.end).unwrap_or(i64::MAX)))
            .await?
            .check()?;
        let rows: Vec<MessageRecord> = response.take(0)?;
        rows.into_iter().map(MessageRecord::into_message).collect()
    }

    async fn delete_messages_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<(Uuid, u64)>> {
        // Compared as datetimes: the stored strings are RFC 3339 but not of
        // one fixed width.
        let mut response = self
            .db
            .query(
                "BEGIN TRANSACTION; \
                 LET $gone = SELECT session_id, seq FROM message \
                     WHERE type::datetime(timestamp) < type::datetime($cutoff); \
                 DELETE message WHERE type::datetime(timestamp) < type::datetime($cutoff); \
                 RETURN $gone; \
                 COMMIT TRANSACTION;",
            )
            .bind(("cutoff", cutoff.to_rfc3339()))
            .await?
            .check()?;
        let rows: Vec<DeletedMessage> = response.take(3)?;
        rows.into_iter()
            .map(|r| Ok((Uuid::parse_str(&r.session_id)?, u64::try_from(r.seq)?)))
            .collect()
    }
}
