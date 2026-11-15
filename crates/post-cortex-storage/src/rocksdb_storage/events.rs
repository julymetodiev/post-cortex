// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The bus's ports ([`EventLog`], [`EventCursors`]) over RocksDB.
//!
//! Key families:
//! - `bus_event:{position}` -> marked bincode [`BusEvent`]; the position is
//!   20 zero-padded digits, so key order is log order.
//! - `bus_name:{name}:{position}` -> nothing: the events of a name, in
//!   order (a name is a-z, 0-9, `_` and `.`).
//! - `bus_next` -> the next position (8 bytes, big endian), kept so a
//!   position is never reused after old events are dropped.
//! - `bus_cursor:{follower}:{name}` -> the position acknowledged (8 bytes,
//!   big endian).
//! - `bus_dropped:{name}` -> the newest position of the name dropped (8
//!   bytes, big endian).
//!
//! An append, a drop and a cursor move are each one `WriteBatch` (or one
//! write) under the store's bus lock.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rocksdb::{DB, WriteBatch};

use post_cortex_core::core::event::{BusEvent, NewBusEvent};
use post_cortex_ports::storage::{EventCursors, EventLog};

use crate::port_error::{PortResult, Result};
use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, scan_prefix};
use crate::rocksdb_storage::rows::{EVENT_V1, decode_marked, encode_marked};

const EVENT_PREFIX: &str = "bus_event:";
const NEXT_KEY: &str = "bus_next";

fn event_key(position: u64) -> String {
    format!("{EVENT_PREFIX}{position:020}")
}

fn name_prefix(name: &str) -> String {
    format!("bus_name:{name}:")
}

fn name_key(name: &str, position: u64) -> String {
    format!("bus_name:{name}:{position:020}")
}

fn cursor_key(follower: &str, name: &str) -> String {
    format!("bus_cursor:{follower}:{name}")
}

fn dropped_key(name: &str) -> String {
    format!("bus_dropped:{name}")
}

/// A big-endian count, 0 when absent.
fn count(db: &DB, key: &str) -> Result<u64> {
    match db.get(key.as_bytes())? {
        None => Ok(0),
        Some(bytes) => {
            let bytes: [u8; 8] = bytes.as_slice().try_into().map_err(|_| {
                post_cortex_ports::error::corrupt("bus count", key, "is not 8 bytes")
            })?;
            Ok(u64::from_be_bytes(bytes))
        }
    }
}

fn decode(key: &[u8], row: &[u8]) -> Result<BusEvent> {
    decode_marked("bus event", &String::from_utf8_lossy(key), EVENT_V1, row)
}

/// The position a `bus_name` key ends with.
fn position_of(key: &[u8]) -> Result<u64> {
    let text = String::from_utf8_lossy(key);
    text.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| {
            post_cortex_ports::error::corrupt("bus name index", &text, "names no position").into()
        })
}

#[async_trait]
impl EventLog for RealRocksDBStorage {
    async fn append_event(&self, event: NewBusEvent) -> PortResult<BusEvent> {
        self.breaker
            .guard(async {
                let (db, lock) = (self.db.clone(), self.bus_lock.clone());
                blocking(move || {
                    let _guard = lock.lock();
                    let position = count(&db, NEXT_KEY)?.max(1);
                    let stored = BusEvent { position, event };
                    let mut batch = WriteBatch::default();
                    batch.put(event_key(position), encode_marked(EVENT_V1, &stored)?);
                    batch.put(name_key(&stored.event.name, position), []);
                    batch.put(NEXT_KEY, (position + 1).to_be_bytes());
                    db.write(batch)?;
                    Ok(stored)
                })
                .await
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
                let db = self.db.clone();
                let name = name.to_string();
                blocking(move || {
                    let mut events = Vec::new();
                    for item in scan_prefix(&db, name_prefix(&name).as_bytes()) {
                        if events.len() == limit {
                            break;
                        }
                        let (key, _) = item?;
                        let position = position_of(&key)?;
                        if position <= after {
                            continue;
                        }
                        let key = event_key(position);
                        // Dropped between the index read and this one.
                        let Some(row) = db.get(key.as_bytes())? else {
                            continue;
                        };
                        events.push(decode(key.as_bytes(), &row)?);
                    }
                    Ok(events)
                })
                .await
            })
            .await
    }

    async fn oldest_event(&self) -> PortResult<Option<u64>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    scan_prefix(&db, EVENT_PREFIX.as_bytes())
                        .next()
                        .transpose()?
                        .map(|(key, row)| decode(&key, &row).map(|e| e.position))
                        .transpose()
                })
                .await
            })
            .await
    }

    async fn drop_events_before(&self, before: DateTime<Utc>) -> PortResult<usize> {
        self.breaker
            .guard(async {
                let (db, lock) = (self.db.clone(), self.bus_lock.clone());
                blocking(move || {
                    let _guard = lock.lock();
                    let mut batch = WriteBatch::default();
                    let mut dropped = 0;
                    let mut through = std::collections::BTreeMap::new();
                    // In log order an event's time only grows, but the host's
                    // clock may step back: every event is looked at.
                    for item in scan_prefix(&db, EVENT_PREFIX.as_bytes()) {
                        let (key, row) = item?;
                        let event = decode(&key, &row)?;
                        if event.event.at < before {
                            batch.delete(&key);
                            batch.delete(name_key(&event.event.name, event.position));
                            through.insert(event.event.name, event.position);
                            dropped += 1;
                        }
                    }
                    // In log order: the last position of each name is its newest.
                    for (name, position) in through {
                        batch.put(dropped_key(&name), position.to_be_bytes());
                    }
                    db.write(batch)?;
                    Ok(dropped)
                })
                .await
            })
            .await
    }

    async fn dropped_through(&self, name: &str) -> PortResult<u64> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let key = dropped_key(name);
                blocking(move || count(&db, &key)).await
            })
            .await
    }
}

#[async_trait]
impl EventCursors for RealRocksDBStorage {
    async fn cursor(&self, follower: &str, name: &str) -> PortResult<u64> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let key = cursor_key(follower, name);
                blocking(move || count(&db, &key)).await
            })
            .await
    }

    async fn advance_cursor(&self, follower: &str, name: &str, position: u64) -> PortResult<()> {
        self.breaker
            .guard(async {
                let (db, lock) = (self.db.clone(), self.bus_lock.clone());
                let key = cursor_key(follower, name);
                blocking(move || {
                    let _guard = lock.lock();
                    if position > count(&db, &key)? {
                        db.put(key.as_bytes(), position.to_be_bytes())?;
                    }
                    Ok(())
                })
                .await
            })
            .await
    }
}
