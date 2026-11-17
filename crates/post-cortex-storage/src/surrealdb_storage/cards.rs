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
//! Every version is a row of the `card` table keyed `{scope}_{version}`
//! (`scope` is [`CardScope::key`]), unique on `(scope, version)`: of two
//! writers that read the same latest version, the second one's insert fails
//! on the index and is reported as a conflict.

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_ports::storage::{CardStore, CardWrite};

use crate::surrealdb_storage::SurrealDBStorage;

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
            scope: post_cortex_ports::error::parse_name("card", &self.scope, &self.scope)?,
            version: u64::try_from(self.version)?,
            text: self.text,
            updated_at: post_cortex_ports::error::parse_timestamp(
                "card",
                &self.scope,
                &self.updated_at,
            )?,
            author: self.author,
        })
    }
}

/// `card` row for the batched upsert.
#[derive(Serialize, SurrealValue)]
struct KeyedCard {
    /// `{scope}_{version}`.
    key: String,
    record: CardRecord,
}

impl SurrealDBStorage {
    async fn latest_cards(&self, scope: CardScope, limit: usize) -> Result<Vec<Card>> {
        let limit = crate::surrealdb_storage::limit::Limit::of(limit);
        let mut response = self
            .db()
            .query(format!(
                "SELECT * OMIT id FROM card WHERE scope = $scope \
                 ORDER BY version DESC {}",
                limit.clause()
            ))
            .bind(("scope", scope.key()))
            .bind(("limit", limit.rows()))
            .await?
            .check()?;
        let rows: Vec<CardRecord> = response.take(0)?;
        rows.into_iter().map(CardRecord::into_card).collect()
    }
}

#[async_trait]
impl CardStore for SurrealDBStorage {
    async fn get_card(&self, scope: CardScope) -> PortResult<Option<Card>> {
        self.breaker
            .guard(async { Ok(self.latest_cards(scope, 1).await?.into_iter().next()) })
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
                    .db()
                    .query("CREATE type::record('card', $key) CONTENT $record RETURN NONE;")
                    .bind(("key", format!("{}_{}", scope.key(), card.version)))
                    .bind(("record", CardRecord::from_card(&card)))
                    .await?
                    .check();
                match created {
                    Ok(_) => Ok(CardWrite::Written(card)),
                    // Another writer stored this version first.
                    Err(e) if e.is_already_exists() => {
                        Ok(CardWrite::Conflict(self.get_card(scope).await?))
                    }
                    Err(e) => Err(e.into()),
                }
            })
            .await
    }

    async fn card_people(&self) -> PortResult<Vec<uuid::Uuid>> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query(
                        "SELECT VALUE scope FROM card \
                         WHERE string::starts_with(scope, 'person:') GROUP BY scope",
                    )
                    .await?
                    .check()?;
                let scopes: Vec<String> = response.take(0)?;
                let mut people = Vec::with_capacity(scopes.len());
                for key in &scopes {
                    match post_cortex_ports::error::parse_name::<CardScope>("card", key, key)? {
                        CardScope::Person(id) => people.push(id),
                        _ => {
                            return Err(post_cortex_ports::error::corrupt(
                                "card",
                                key,
                                "no person's card",
                            )
                            .into());
                        }
                    }
                }
                Ok(people)
            })
            .await
    }

    async fn card_history(&self, scope: CardScope, limit: usize) -> PortResult<Vec<Card>> {
        self.breaker
            .guard(async { self.latest_cards(scope, limit).await })
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
                let rows: Vec<KeyedCard> = versions
                    .into_iter()
                    .map(|card| {
                        let card = Card { scope, ..card };
                        KeyedCard {
                            key: format!("{}_{}", scope.key(), card.version),
                            record: CardRecord::from_card(&card),
                        }
                    })
                    .collect();
                let clear = if replace {
                    "DELETE card WHERE scope = $scope;"
                } else {
                    ""
                };
                self.db()
                    .query(format!(
                        "BEGIN TRANSACTION; {clear} \
                 FOR $c IN $rows {{ UPSERT type::record('card', $c.key) CONTENT $c.record; }}; \
                 COMMIT TRANSACTION;"
                    ))
                    .bind(("scope", scope.key()))
                    .bind(("rows", rows))
                    .await?
                    .check()?;
                Ok(())
            })
            .await
    }
}
