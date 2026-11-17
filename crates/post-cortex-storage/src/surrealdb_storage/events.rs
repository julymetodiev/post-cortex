// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The bus's ports ([`EventLog`], [`EventCursors`]) over SurrealDB.
//!
//! An event is a `bus_event` row keyed by its position: an append reads
//! the next position from `bus_next:log` and creates the row with it and
//! the next one in one transaction; a create of a taken key fails, and the
//! append tries the position after. A cursor is a `bus_cursor` row keyed
//! `{follower}|{name}`, raised only.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

use post_cortex_core::core::event::{BusEvent, NewBusEvent};
use post_cortex_ports::error::{corrupt, parse_timestamp, parse_uuid};
use post_cortex_ports::storage::{EventCursors, EventLog};

use crate::port_error::{PortResult, Result};
use crate::surrealdb_storage::SurrealDBStorage;

/// Appends that find their position taken, one after another, before the
/// append fails.
const TRIES: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct EventRecord {
    position: i64,
    event_id: String,
    name: String,
    module: String,
    workspace_id: Option<String>,
    author: Option<String>,
    at: String,
    at_ns: i64,
    payload_type: String,
    payload: Vec<u8>,
}

fn nanos(at: DateTime<Utc>) -> Result<i64> {
    at.timestamp_nanos_opt()
        .ok_or_else(|| corrupt("bus event", at, "is outside the years 1677 to 2262").into())
}

impl EventRecord {
    fn of(event: &BusEvent) -> Result<Self> {
        let e = &event.event;
        Ok(Self {
            position: i64::try_from(event.position)?,
            event_id: e.id.to_string(),
            name: e.name.clone(),
            module: e.module.clone(),
            workspace_id: e.workspace_id.map(|w| w.to_string()),
            author: e.author.clone(),
            at: e.at.to_rfc3339(),
            at_ns: nanos(e.at)?,
            payload_type: e.payload_type.clone(),
            payload: e.payload.clone(),
        })
    }

    fn into_event(self) -> Result<BusEvent> {
        let kind = "bus event";
        let position = u64::try_from(self.position)?;
        Ok(BusEvent {
            position,
            event: NewBusEvent {
                id: parse_uuid(kind, position, &self.event_id)?,
                name: self.name,
                module: self.module,
                workspace_id: self
                    .workspace_id
                    .map(|w| parse_uuid(kind, position, &w))
                    .transpose()?,
                author: self.author,
                at: parse_timestamp(kind, position, &self.at)?,
                payload_type: self.payload_type,
                payload: self.payload,
            },
        })
    }
}

fn cursor_key(follower: &str, name: &str) -> String {
    format!("{follower}|{name}")
}

impl SurrealDBStorage {
    /// The next position `bus_next:log` names; 1 for an empty log.
    async fn next_position(&self) -> Result<u64> {
        let mut response = self
            .db()
            .query("SELECT VALUE next FROM ONLY bus_next:log;")
            .await?
            .check()?;
        let next: Option<i64> = response.take(0)?;
        Ok(u64::try_from(next.unwrap_or(1))?.max(1))
    }
}

#[async_trait]
impl EventLog for SurrealDBStorage {
    async fn append_event(&self, event: NewBusEvent) -> PortResult<BusEvent> {
        self.breaker
            .guard(async {
                let mut position = self.next_position().await?;
                for _ in 0..TRIES {
                    let stored = BusEvent {
                        position,
                        event: event.clone(),
                    };
                    let created = self
                        .db()
                        .query(
                            "BEGIN TRANSACTION; \
                     CREATE type::record('bus_event', $position) CONTENT $record RETURN NONE; \
                     UPSERT bus_next:log SET next = math::max([next ?? 1, $next]); \
                     COMMIT TRANSACTION;",
                        )
                        .bind(("position", i64::try_from(position)?))
                        .bind(("record", EventRecord::of(&stored)?))
                        .bind(("next", i64::try_from(position + 1)?))
                        .await?;
                    match crate::surrealdb_storage::transaction::committed(created) {
                        Ok(()) => return Ok(stored),
                        // Another append took it: the next one is free.
                        Err(e) if crate::surrealdb_storage::transaction::lost_race(&e) => {
                            position += 1;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                // Appends at once beyond any count a host makes: try again.
                Err(post_cortex_ports::PortError::Conflict(format!(
                    "no free position for an event after {TRIES} tries from {position}"
                ))
                .into())
            })
            .await
    }

    async fn events_after(
        &self,
        name: &str,
        after: u64,
        limit: usize,
    ) -> PortResult<Vec<BusEvent>> {
        self.breaker
            .guard(async {
                let limit = crate::surrealdb_storage::limit::Limit::of(limit);
                let mut response = self
                    .db()
                    .query(format!(
                        "SELECT * OMIT id FROM bus_event WHERE name = $name AND position > $after \
                 ORDER BY position {};",
                        limit.clause()
                    ))
                    .bind(("name", name.to_string()))
                    .bind(("after", i64::try_from(after)?))
                    .bind(("limit", limit.rows()))
                    .await?
                    .check()?;
                let rows: Vec<EventRecord> = response.take(0)?;
                rows.into_iter().map(EventRecord::into_event).collect()
            })
            .await
    }

    async fn oldest_event(&self) -> PortResult<Option<u64>> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query("SELECT VALUE position FROM bus_event ORDER BY position LIMIT 1;")
                    .await?
                    .check()?;
                let oldest: Vec<i64> = response.take(0)?;
                Ok(oldest.first().map(|p| u64::try_from(*p)).transpose()?)
            })
            .await
    }

    async fn drop_events_before(&self, before: DateTime<Utc>) -> PortResult<usize> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query(
                        "BEGIN TRANSACTION; \
                 LET $gone = (DELETE bus_event WHERE at_ns < $before RETURN BEFORE); \
                 FOR $name IN array::distinct($gone.name) { \
                     UPSERT type::record('bus_dropped', $name) SET position = math::max( \
                         [position ?? 0, math::max((SELECT VALUE position FROM $gone WHERE name = $name))]); \
                 }; \
                 RETURN count($gone); \
                 COMMIT TRANSACTION;",
                    )
                    .bind(("before", nanos(before)?))
                    .await?
                    .check()?;
                let dropped: Option<i64> = response.take(3)?;
                Ok(usize::try_from(dropped.unwrap_or(0))?)
            })
            .await
    }

    async fn dropped_through(&self, name: &str) -> PortResult<u64> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query("SELECT VALUE position FROM ONLY type::record('bus_dropped', $name);")
                    .bind(("name", name.to_string()))
                    .await?
                    .check()?;
                let position: Option<i64> = response.take(0)?;
                Ok(u64::try_from(position.unwrap_or(0))?)
            })
            .await
    }
}

#[async_trait]
impl EventCursors for SurrealDBStorage {
    async fn cursor(&self, follower: &str, name: &str) -> PortResult<u64> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query("SELECT VALUE position FROM ONLY type::record('bus_cursor', $key);")
                    .bind(("key", cursor_key(follower, name)))
                    .await?
                    .check()?;
                let position: Option<i64> = response.take(0)?;
                Ok(u64::try_from(position.unwrap_or(0))?)
            })
            .await
    }

    async fn advance_cursor(&self, follower: &str, name: &str, position: u64) -> PortResult<()> {
        self.breaker
            .guard(async {
                self.db()
                    .query(
                        "UPSERT type::record('bus_cursor', $key) \
                 SET position = math::max([position ?? 0, $position]) RETURN NONE;",
                    )
                    .bind(("key", cursor_key(follower, name)))
                    .bind(("position", i64::try_from(position)?))
                    .await?
                    .check()?;
                Ok(())
            })
            .await
    }
}
