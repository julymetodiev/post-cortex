// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Project and user cards (ROADMAP Phase 2 item 3).
//!
//! A card is a small markdown document that is always put in front of the
//! agent: one per project (workspace) with its stack, conventions and
//! current goals, and one global card with the user's preferences. Every
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
    /// The global card of the user.
    User,
}

impl CardScope {
    /// Stable key of the scope in storage: `workspace:<uuid>` or `user`.
    pub fn key(&self) -> String {
        match self {
            Self::Workspace(id) => format!("workspace:{id}"),
            Self::User => "user".to_string(),
        }
    }

    /// Parse [`Self::key`].
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "user" => Some(Self::User),
            _ => key
                .strip_prefix("workspace:")
                .and_then(|id| Uuid::parse_str(id).ok())
                .map(Self::Workspace),
        }
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
        assert_eq!(CardScope::from_key(&ws.key()), Some(ws));
        assert_eq!(CardScope::from_key("user"), Some(CardScope::User));
        assert_eq!(CardScope::from_key("workspace:nope"), None);
    }
}
