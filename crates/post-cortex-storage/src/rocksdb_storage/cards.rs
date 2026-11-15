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

//! Project and user cards and the [`CardStore`] trait implementation.
//!
//! Key families:
//! - `card:{scope}:{version}` -> marked bincode [`Card`]; `scope` is
//!   [`CardScope::key`], `version` 20 zero-padded digits, so key order is
//!   version order.
//! - `card_head:{scope}` -> the latest version (8 bytes, big endian).
//!
//! A write is one `WriteBatch` under the lock of the scope (a workspace's
//! or a person's id; the nil id for the host's user card).

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use chrono::Utc;
use rocksdb::{DB, Direction, IteratorMode, ReadOptions, WriteBatch};
use uuid::Uuid;

use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_ports::error::corrupt;
use post_cortex_ports::storage::{CardStore, CardWrite};

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, keys_with_prefix};
use crate::rocksdb_storage::rows::{CARD_V1, decode_marked, encode_marked};

fn card_prefix(scope: CardScope) -> String {
    format!("card:{}:", scope.key())
}

fn card_key(scope: CardScope, version: u64) -> String {
    format!("card:{}:{:020}", scope.key(), version)
}

fn head_key(scope: CardScope) -> String {
    format!("card_head:{}", scope.key())
}

/// The prefix of the heads of the persons' user cards.
const PERSON_HEADS: &str = "card_head:person:";

fn lock_id(scope: CardScope) -> Uuid {
    match scope {
        CardScope::Workspace(id) | CardScope::Person(id) => id,
        CardScope::User => Uuid::nil(),
    }
}

fn encode_card(card: &Card) -> Result<Vec<u8>> {
    encode_marked(CARD_V1, card)
}

fn decode_card(key: &[u8], row: &[u8]) -> Result<Card> {
    decode_marked("card", &String::from_utf8_lossy(key), CARD_V1, row)
}

fn head(db: &DB, scope: CardScope) -> Result<u64> {
    match db.get(head_key(scope).as_bytes())? {
        Some(bytes) => {
            let raw: [u8; 8] = bytes.as_slice().try_into().map_err(|_| {
                corrupt(
                    "card head",
                    head_key(scope),
                    format!("{} bytes", bytes.len()),
                )
            })?;
            Ok(u64::from_be_bytes(raw))
        }
        None => Ok(0),
    }
}

fn load(db: &DB, scope: CardScope, version: u64) -> Result<Option<Card>> {
    if version == 0 {
        return Ok(None);
    }
    let key = card_key(scope, version);
    db.get(key.as_bytes())?
        .map(|bytes| decode_card(key.as_bytes(), &bytes))
        .transpose()
}

/// Delete the card of `scope` and its history in `batch`.
pub fn delete_card_in(db: &DB, batch: &mut WriteBatch, scope: CardScope) -> Result<()> {
    batch.delete(head_key(scope).as_bytes());
    for key in keys_with_prefix(db, card_prefix(scope).as_bytes())? {
        batch.delete(&key);
    }
    Ok(())
}

#[async_trait]
impl CardStore for RealRocksDBStorage {
    async fn get_card(&self, scope: CardScope) -> PortResult<Option<Card>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || load(&db, scope, head(&db, scope)?)).await
            })
            .await
    }

    async fn put_card(
        &self,
        scope: CardScope,
        text: String,
        author: Option<String>,
        expected_version: u64,
    ) -> PortResult<CardWrite> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(lock_id(scope));
                    let latest = head(&db, scope)?;
                    if latest != expected_version {
                        return Ok(CardWrite::Conflict(load(&db, scope, latest)?));
                    }
                    let card = Card {
                        scope,
                        version: latest + 1,
                        text,
                        updated_at: Utc::now(),
                        author,
                    };
                    let mut batch = WriteBatch::default();
                    batch.put(
                        card_key(scope, card.version).as_bytes(),
                        encode_card(&card)?,
                    );
                    batch.put(head_key(scope).as_bytes(), card.version.to_be_bytes());
                    db.write(batch)?;
                    Ok(CardWrite::Written(card))
                })
                .await
            })
            .await
    }

    async fn card_people(&self) -> PortResult<Vec<Uuid>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let mut people = Vec::new();
                    for key in keys_with_prefix(&db, PERSON_HEADS.as_bytes())? {
                        let key = String::from_utf8_lossy(&key).into_owned();
                        let person = key
                            .strip_prefix("card_head:")
                            .and_then(|scope| scope.parse::<CardScope>().ok());
                        match person {
                            Some(CardScope::Person(id)) => people.push(id),
                            _ => return Err(corrupt("card head", key, "no person's card").into()),
                        }
                    }
                    Ok(people)
                })
                .await
            })
            .await
    }

    async fn card_history(&self, scope: CardScope, limit: usize) -> PortResult<Vec<Card>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let prefix = card_prefix(scope);
                    let mut opts = ReadOptions::default();
                    opts.set_total_order_seek(true);
                    opts.set_iterate_lower_bound(prefix.clone().into_bytes());
                    // Seek from just past the prefix, walking back.
                    let mut upper = prefix.clone().into_bytes();
                    upper.push(0xFF);
                    let mut cards = Vec::new();
                    for item in
                        db.iterator_opt(IteratorMode::From(&upper, Direction::Reverse), opts)
                    {
                        if cards.len() >= limit {
                            break;
                        }
                        let (key, value) = item?;
                        if !key.starts_with(prefix.as_bytes()) {
                            break;
                        }
                        cards.push(decode_card(&key, &value)?);
                    }
                    Ok(cards)
                })
                .await
            })
            .await
    }

    async fn restore_card_versions(
        &self,
        scope: CardScope,
        versions: Vec<Card>,
        replace: bool,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                if versions.is_empty() && !replace {
                    return Ok(());
                }
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(lock_id(scope));
                    let mut batch = WriteBatch::default();
                    let mut latest = 0;
                    if replace {
                        delete_card_in(&db, &mut batch, scope)?;
                    } else {
                        latest = head(&db, scope)?;
                    }
                    for mut card in versions {
                        card.scope = scope;
                        latest = latest.max(card.version);
                        batch.put(
                            card_key(scope, card.version).as_bytes(),
                            encode_card(&card)?,
                        );
                    }
                    if latest > 0 {
                        batch.put(head_key(scope).as_bytes(), latest.to_be_bytes());
                    }
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }
}
