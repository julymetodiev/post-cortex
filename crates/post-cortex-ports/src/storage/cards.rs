// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The card port.

use crate::error::Result;
use async_trait::async_trait;
use post_cortex_core::core::card::{Card, CardScope};

/// Outcome of [`CardStore::put_card`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardWrite {
    /// The text was stored as this new version.
    Written(Card),
    /// The latest stored version was not the expected one; nothing was
    /// written. Carries the latest version (`None`: no card yet).
    Conflict(Option<Card>),
}

/// Storage for project and user cards (see
/// [`post_cortex_core::core::card`]). Every write is a new version.
#[async_trait]
pub trait CardStore: Send + Sync {
    /// The latest version of the card of `scope`, if it has one.
    async fn get_card(&self, scope: CardScope) -> Result<Option<Card>>;

    /// Store `text` as the next version of the card of `scope`, if its
    /// latest version is `expected_version` (0: no card yet).
    async fn put_card(
        &self,
        scope: CardScope,
        text: String,
        author: Option<String>,
        expected_version: u64,
    ) -> Result<CardWrite>;

    /// The persons who have a user card ([`CardScope::Person`]), in no
    /// order: what a full export carries with the host's card.
    async fn card_people(&self) -> Result<Vec<uuid::Uuid>>;

    /// Up to `limit` versions of the card of `scope`, newest first.
    async fn card_history(&self, scope: CardScope, limit: usize) -> Result<Vec<Card>>;

    /// Store `versions` of the card of `scope` as they are, with their
    /// version numbers, times and authors (an import), after deleting the
    /// stored history when `replace`. The highest version stored becomes
    /// the latest.
    async fn restore_card_versions(
        &self,
        scope: CardScope,
        versions: Vec<Card>,
        replace: bool,
    ) -> Result<()>;
}
