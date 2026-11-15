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
//! Rows live in the `message` table keyed `{session_id}_{seq}`, unique on
//! `(session_id, seq)` and on `(session_id, turn_id)`. The next position of
//! each stream is kept in `message_next` (keyed by session id), so
//! positions are never reused after retention empties a stream. An append
//! writes its rows, the new position and the stream's summary
//! ([`summary`]) in one transaction; a delete recounts the summary in its.

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use post_cortex_core::core::message::{
    Message, MessageCursor, MessageOrder, MessageStats, NewMessage,
};
use post_cortex_ports::storage::MessageStore;

use crate::surrealdb_storage::SurrealDBStorage;

mod summary;

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
    parent: Option<String>,
    /// The blocks as JSON (`message_blocks`).
    blocks: Option<String>,
}

impl MessageRecord {
    fn from_message(message: &Message) -> Result<Self> {
        Ok(Self {
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
            parent: message.parent.clone(),
            blocks: crate::message_blocks::encode(&message.turn_id, &message.blocks)?,
        })
    }

    fn into_message(self) -> Result<Message> {
        let blocks = crate::message_blocks::decode(&self.turn_id, self.blocks.as_deref())?;
        Ok(Message {
            session_id: post_cortex_ports::error::parse_uuid(
                "message",
                &self.turn_id,
                &self.session_id,
            )?,
            seq: u64::try_from(self.seq).map_err(|e| {
                post_cortex_ports::error::corrupt("message", &self.turn_id, format!("seq: {e}"))
            })?,
            role: post_cortex_ports::error::parse_name("message", &self.turn_id, &self.role)?,
            turn_id: self.turn_id,
            author: self.author,
            source_session: self.source_session,
            timestamp: post_cortex_ports::error::parse_timestamp(
                "message",
                &self.session_id,
                &self.timestamp,
            )?,
            text: self.text,
            tools: self.tools,
            files: self.files,
            parent: self.parent,
            blocks,
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
impl MessageStore for SurrealDBStorage {
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
                let sid = session_id.to_string();
                let turns: Vec<String> = messages.iter().map(|m| m.turn_id.clone()).collect();
                let mut response = self
                    .db()
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
                let state = state.ok_or(crate::port_error::AdapterError::NoRow(
                    "message stream state",
                ))?;

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

                let rows = appended
                    .iter()
                    .map(|m| {
                        Ok(KeyedMessage {
                            key: format!("{}_{}", sid, m.seq),
                            record: MessageRecord::from_message(m)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let (first, last) = (&appended[0], &appended[appended.len() - 1]);
                let authors: Vec<String> =
                    appended.iter().filter_map(|m| m.author.clone()).collect();
                self.db()
                    .query(format!(
                        "BEGIN TRANSACTION; \
                 FOR $m IN $rows {{ CREATE type::record('message', $m.key) CONTENT $m.record; }}; \
                 UPSERT type::record('message_next', $sid) SET next = $next; \
                 {} \
                 COMMIT TRANSACTION;",
                        summary::add_append()
                    ))
                    .bind(("rows", rows))
                    .bind(("next", next as i64))
                    .bind(("added", appended.len() as i64))
                    .bind(("first_seq", i64::try_from(first.seq)?))
                    .bind(("first_at", first.timestamp.to_rfc3339()))
                    .bind(("last_seq", i64::try_from(last.seq)?))
                    .bind(("last_at", last.timestamp.to_rfc3339()))
                    .bind(("authors", authors))
                    .bind(("sid", sid))
                    .await?
                    .check()?;
                Ok(appended)
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
                let sid = session_id.to_string();
                let turns: Vec<String> = messages.iter().map(|m| m.turn_id.clone()).collect();
                let mut response = self
                    .db()
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
                let state = state.ok_or(crate::port_error::AdapterError::NoRow(
                    "message stream state",
                ))?;
                let next = u64::try_from(state.next.unwrap_or(0))?;
                let known: HashSet<String> = state.turns.into_iter().collect();
                let (placed, next) = crate::export_import::place_restored_messages(
                    session_id, messages, next, &known,
                );
                if placed.is_empty() {
                    return Ok(placed);
                }

                let rows = placed
                    .iter()
                    .map(|(_, m)| {
                        Ok(KeyedMessage {
                            key: format!("{}_{}", sid, m.seq),
                            record: MessageRecord::from_message(m)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.db()
                    .query(format!(
                        "BEGIN TRANSACTION; \
                 FOR $m IN $rows {{ CREATE type::record('message', $m.key) CONTENT $m.record; }}; \
                 UPSERT type::record('message_next', $sid) SET next = $next; \
                 {} \
                 COMMIT TRANSACTION;",
                        summary::recount("[$sid]")
                    ))
                    .bind(("rows", rows))
                    .bind(("sid", sid))
                    .bind(("next", i64::try_from(next).unwrap_or(i64::MAX)))
                    .await?
                    .check()?;
                Ok(placed)
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
                let mut response = self
                    .db()
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
            })
            .await
    }

    async fn message_stats(&self, session_id: Uuid) -> PortResult<MessageStats> {
        self.breaker
            .guard(async {
                let mut stats = summary::stats_of(&self.db(), &[session_id]).await?;
                stats.pop().ok_or(crate::port_error::AdapterError::NoRow(
                    "message stream summary",
                ))
            })
            .await
    }

    async fn message_stats_of(&self, session_ids: &[Uuid]) -> PortResult<Vec<MessageStats>> {
        self.breaker
            .guard(summary::stats_of(&self.db(), session_ids))
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
                let (bound, seq) = match cursor {
                    MessageCursor::From(seq) => (">=", seq),
                    MessageCursor::Before(seq) => ("<", seq),
                };
                let direction = match order {
                    MessageOrder::Asc => "ASC",
                    MessageOrder::Desc => "DESC",
                };
                let mut response = self
                    .db()
                    .query(format!(
                        "SELECT * OMIT id FROM message \
                 WHERE session_id = $sid AND seq {bound} $seq \
                 ORDER BY seq {direction} LIMIT $limit"
                    ))
                    .bind(("sid", session_id.to_string()))
                    // Past the largest stored int: `From` matches nothing, `Before` all.
                    .bind(("seq", i64::try_from(seq).unwrap_or(i64::MAX)))
                    .bind(("limit", i64::try_from(limit).unwrap_or(i64::MAX)))
                    .await?
                    .check()?;
                let rows: Vec<MessageRecord> = response.take(0)?;
                rows.into_iter().map(MessageRecord::into_message).collect()
            })
            .await
    }

    async fn delete_message(&self, session_id: Uuid, seq: u64) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query(format!(
                        "BEGIN TRANSACTION; \
                 LET $gone = SELECT VALUE seq FROM message WHERE session_id = $sid AND seq = $seq; \
                 DELETE message WHERE session_id = $sid AND seq = $seq; \
                 {} \
                 RETURN $gone; \
                 COMMIT TRANSACTION;",
                        summary::recount("[$sid]")
                    ))
                    .bind(("sid", session_id.to_string()))
                    .bind(("seq", i64::try_from(seq).unwrap_or(i64::MAX)))
                    .await?
                    .check()?;
                let gone: Vec<i64> = response.take(4)?;
                Ok(!gone.is_empty())
            })
            .await
    }

    async fn delete_session_messages(&self, session_id: Uuid) -> PortResult<Vec<u64>> {
        self.breaker
            .guard(async {
                // `message_next` stays, so positions are not reused.
                let mut response = self
                    .db()
                    .query(format!(
                        "BEGIN TRANSACTION; \
                 LET $gone = SELECT VALUE seq FROM message WHERE session_id = $sid; \
                 DELETE message WHERE session_id = $sid; \
                 {} \
                 RETURN $gone; \
                 COMMIT TRANSACTION;",
                        summary::recount("[$sid]")
                    ))
                    .bind(("sid", session_id.to_string()))
                    .await?
                    .check()?;
                let gone: Vec<i64> = response.take(4)?;
                gone.into_iter()
                    .map(|seq| Ok(u64::try_from(seq)?))
                    .collect()
            })
            .await
    }

    async fn delete_messages_before(&self, cutoff: DateTime<Utc>) -> PortResult<Vec<(Uuid, u64)>> {
        self.breaker
            .guard(async {
                // Compared as datetimes: the stored strings are RFC 3339 but not of
                // one fixed width.
                let mut response = self
                    .db()
                    .query(format!(
                        "BEGIN TRANSACTION; \
                 LET $gone = SELECT session_id, seq FROM message \
                     WHERE type::datetime(timestamp) < type::datetime($cutoff); \
                 DELETE message WHERE type::datetime(timestamp) < type::datetime($cutoff); \
                 {} \
                 RETURN $gone; \
                 COMMIT TRANSACTION;",
                        summary::recount("array::distinct($gone.session_id)")
                    ))
                    .bind(("cutoff", cutoff.to_rfc3339()))
                    .await?
                    .check()?;
                let rows: Vec<DeletedMessage> = response.take(4)?;
                rows.into_iter()
                    .map(|r| {
                        let session = post_cortex_ports::error::parse_uuid(
                            "message",
                            &r.session_id,
                            &r.session_id,
                        )?;
                        Ok((session, u64::try_from(r.seq)?))
                    })
                    .collect()
            })
            .await
    }
}
