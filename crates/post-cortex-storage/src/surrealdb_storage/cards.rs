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
//! Every version is a row of the `card` table keyed `{scope}_{version}`
//! (`scope` is [`CardScope::key`]), unique on `(scope, version)`: of two
//! writers that read the same latest version, the second one's insert fails
//! on the index and is reported as a conflict.

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

use crate::traits::{CardStorage, CardWrite};
use post_cortex_core::core::card::{Card, CardScope};

use super::SurrealDBStorage;

/// Row of the `card` table.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct CardRecord {
    scope: String,
    version: i64,
    text: String,
    updated_at: String,
    author: Option<String>,
}

impl CardRecord {
    fn from_card(card: &Card) -> Self {
        Self {
            scope: card.scope.key(),
            version: card.version as i64,
            text: card.text.clone(),
            updated_at: card.updated_at.to_rfc3339(),
            author: card.author.clone(),
        }
    }

    fn into_card(self) -> Result<Card> {
        Ok(Card {
            scope: CardScope::from_key(&self.scope)
                .ok_or_else(|| anyhow::anyhow!("Unknown card scope '{}'", self.scope))?,
            version: u64::try_from(self.version)?,
            text: self.text,
            updated_at: SurrealDBStorage::parse_datetime(&self.updated_at),
            author: self.author,
        })
    }
}

impl SurrealDBStorage {
    async fn latest_cards(&self, scope: CardScope, limit: usize) -> Result<Vec<Card>> {
        let mut response = self
            .db
            .query(
                "SELECT * OMIT id FROM card WHERE scope = $scope \
                 ORDER BY version DESC LIMIT $limit",
            )
            .bind(("scope", scope.key()))
            .bind(("limit", limit as i64))
            .await?
            .check()?;
        let rows: Vec<CardRecord> = response.take(0)?;
        rows.into_iter().map(CardRecord::into_card).collect()
    }
}

#[async_trait]
impl CardStorage for SurrealDBStorage {
    async fn get_card(&self, scope: CardScope) -> Result<Option<Card>> {
        Ok(self.latest_cards(scope, 1).await?.into_iter().next())
    }

    async fn put_card(
        &self,
        scope: CardScope,
        text: String,
        author: Option<String>,
        expected_version: u64,
    ) -> Result<CardWrite> {
        let latest = self.get_card(scope).await?;
        if latest.as_ref().map_or(0, |c| c.version) != expected_version {
            return Ok(CardWrite::Conflict(latest));
        }
        let card = Card {
            scope,
            version: expected_version + 1,
            text,
            updated_at: Utc::now(),
            author,
        };
        let created = self
            .db
            .query("CREATE type::record('card', $key) CONTENT $record RETURN NONE;")
            .bind(("key", format!("{}_{}", scope.key(), card.version)))
            .bind(("record", CardRecord::from_card(&card)))
            .await?
            .check();
        match created {
            Ok(_) => Ok(CardWrite::Written(card)),
            // Another writer stored this version first.
            Err(e) if e.to_string().contains("already") => {
                Ok(CardWrite::Conflict(self.get_card(scope).await?))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn card_history(&self, scope: CardScope, limit: usize) -> Result<Vec<Card>> {
        self.latest_cards(scope, limit).await
    }
}
