// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Project and user cards.
//!
//! A card is a small markdown document that is always put in front of the
//! agent: one per project (workspace) with its stack, conventions and
//! current goals, and one card with a person's preferences: the user card
//! (one per person of an organisation, one for a local host). Every
//! change is a new version; the history is kept. Writers name the version
//! they edited, so two agents editing at once cannot overwrite each other.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Longest card text, in bytes: cards are injected into every session.
pub const MAX_CARD_BYTES: usize = 2048;

/// Whose card it is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum CardScope {
    /// The card of one project (workspace).
    Workspace(Uuid),
    /// The user card of a host without persons (a local daemon).
    User,
    /// The user card of one person (a cloud user's id): what a call for that
    /// person reads and writes as the user card.
    Person(Uuid),
}

impl CardScope {
    /// Stable key of the scope in storage: `workspace:<uuid>`, `user` or
    /// `person:<uuid>` (not `user:`, which a store's prefix of the host's
    /// card would match).
    pub fn key(&self) -> String {
        match self {
            Self::Workspace(id) => format!("workspace:{id}"),
            Self::User => "user".to_string(),
            Self::Person(id) => format!("person:{id}"),
        }
    }

    /// The user card of a call for `person`: that person's, else the
    /// host's own when the call is for none.
    #[must_use]
    pub fn user_of(person: Option<Uuid>) -> Self {
        person.map_or(Self::User, Self::Person)
    }

    /// Whether it is a user card (a person's or the host's).
    #[must_use]
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User | Self::Person(_))
    }
}

/// A call's person (`x-pcx-person`) that is no user id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the call's person '{0}' is no user id")]
pub struct NotAPerson(pub String);

/// The person a call is for, as its `x-pcx-person` names it: a user id;
/// none or empty: no person.
///
/// # Errors
/// [`NotAPerson`] for a value that is no UUID.
pub fn person(raw: Option<&str>) -> Result<Option<Uuid>, NotAPerson> {
    match raw.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => Uuid::parse_str(v)
            .map(Some)
            .map_err(|_| NotAPerson(v.to_string())),
    }
}

/// Strict inverse of [`CardScope::key`].
impl std::str::FromStr for CardScope {
    type Err = crate::core::wire::UnknownVariant;

    fn from_str(key: &str) -> Result<Self, Self::Err> {
        if key == "user" {
            return Ok(Self::User);
        }
        let id = |prefix: &str| {
            key.strip_prefix(prefix)
                .and_then(|id| Uuid::parse_str(id).ok())
        };
        id("workspace:")
            .map(Self::Workspace)
            .or_else(|| id("person:").map(Self::Person))
            .ok_or_else(|| crate::core::wire::UnknownVariant {
                kind: "card scope",
                value: key.to_string(),
            })
    }
}

/// One version of a card.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Whose card it is.
    pub scope: CardScope,
    /// 1 for the first text, one more for every change.
    pub version: u64,
    /// The markdown text, at most [`MAX_CARD_BYTES`].
    pub text: String,
    /// When this version was written.
    pub updated_at: DateTime<Utc>,
    /// Who wrote this version (agent name or user), when known.
    pub author: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_keys_round_trip() {
        let ws = CardScope::Workspace(Uuid::new_v4());
        assert_eq!(ws.key().parse::<CardScope>(), Ok(ws));
        assert_eq!("user".parse::<CardScope>(), Ok(CardScope::User));
        let person = CardScope::Person(Uuid::new_v4());
        assert_eq!(person.key().parse::<CardScope>(), Ok(person));
        assert_eq!(CardScope::user_of(None), CardScope::User);
        assert!("workspace:nope".parse::<CardScope>().is_err());
        assert!("person:nope".parse::<CardScope>().is_err());
    }
}
