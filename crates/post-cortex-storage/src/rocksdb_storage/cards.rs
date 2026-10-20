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

//! Project and user cards and the [`CardStorage`] trait implementation.
//!
//! Key families:
//! - `card:{scope}:{version}` -> bincode [`Card`]; `scope` is
//!   [`CardScope::key`], `version` 20 zero-padded digits, so key order is
//!   version order.
//! - `card_head:{scope}` -> the latest version (8 bytes, big endian).
//!
//! A write is one `WriteBatch` under the lock of the scope (a workspace's
//! id; the nil id for the user card).

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rocksdb::{DB, Direction, IteratorMode, ReadOptions, WriteBatch};
use uuid::Uuid;

use crate::traits::{CardStorage, CardWrite};
use post_cortex_core::core::card::{Card, CardScope};

use super::RealRocksDBStorage;
use super::core::{blocking, keys_with_prefix};

fn card_prefix(scope: CardScope) -> String {
    format!("card:{}:", scope.key())
}

fn card_key(scope: CardScope, version: u64) -> String {
    format!("card:{}:{:020}", scope.key(), version)
}

fn head_key(scope: CardScope) -> String {
    format!("card_head:{}", scope.key())
}

fn lock_id(scope: CardScope) -> Uuid {
    match scope {
        CardScope::Workspace(id) => id,
        CardScope::User => Uuid::nil(),
    }
}

fn encode_card(card: &Card) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(card, bincode::config::standard())
        .map_err(|e| anyhow::anyhow!("Failed to serialize card: {}", e))
}

fn decode_card(bytes: &[u8]) -> Result<Card> {
    bincode::serde::decode_from_slice::<Card, _>(bytes, bincode::config::standard())
        .map(|(c, _)| c)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize card: {}", e))
}

fn head(db: &DB, scope: CardScope) -> Result<u64> {
    match db.get(head_key(scope).as_bytes())? {
        Some(bytes) => {
            let raw: [u8; 8] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("Malformed card head ({} bytes)", bytes.len()))?;
            Ok(u64::from_be_bytes(raw))
        }
        None => Ok(0),
    }
}

fn load(db: &DB, scope: CardScope, version: u64) -> Result<Option<Card>> {
    if version == 0 {
        return Ok(None);
    }
    db.get(card_key(scope, version).as_bytes())?
        .map(|bytes| decode_card(&bytes))
        .transpose()
}

/// Delete the card of `scope` and its history in `batch`.
pub(super) fn delete_card_in(db: &DB, batch: &mut WriteBatch, scope: CardScope) -> Result<()> {
    batch.delete(head_key(scope).as_bytes());
    for key in keys_with_prefix(db, card_prefix(scope).as_bytes())? {
        batch.delete(&key);
    }
    Ok(())
}

#[async_trait]
impl CardStorage for RealRocksDBStorage {
    async fn get_card(&self, scope: CardScope) -> Result<Option<Card>> {
        let db = self.db.clone();
        blocking(move || load(&db, scope, head(&db, scope)?)).await
    }

    async fn put_card(
        &self,
        scope: CardScope,
        text: String,
        author: Option<String>,
        expected_version: u64,
    ) -> Result<CardWrite> {
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
    }

    async fn card_history(&self, scope: CardScope, limit: usize) -> Result<Vec<Card>> {
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
            for item in db.iterator_opt(IteratorMode::From(&upper, Direction::Reverse), opts) {
                if cards.len() >= limit {
                    break;
                }
                let (key, value) = item?;
                if !key.starts_with(prefix.as_bytes()) {
                    break;
                }
                cards.push(decode_card(&value)?);
            }
            Ok(cards)
        })
        .await
    }
}
