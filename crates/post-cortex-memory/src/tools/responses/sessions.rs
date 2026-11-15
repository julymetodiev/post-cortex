// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Session and message responses.

use post_cortex_ports::SessionSummary;
use serde::Serialize;

use crate::tools::responses::rfc3339;

/// Every stored session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionList {
    /// Oldest first.
    pub sessions: Vec<SessionListing>,
}

/// A session of a [`SessionList`].
#[derive(Debug, Clone, Serialize)]
pub struct SessionListing {
    /// Its id.
    pub id: String,
    /// Its name.
    pub name: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// Creation time.
    pub created_at: String,
    /// Time of its latest change.
    pub last_updated: String,
    /// Its records.
    pub update_count: usize,
    /// Its entities; null unless the session is in memory.
    pub entity_count: Option<usize>,
}

impl SessionListing {
    /// The listing of `summary`; `entity_count` when known.
    #[must_use]
    pub fn new(summary: &SessionSummary, entity_count: Option<usize>) -> Self {
        Self {
            id: summary.id.to_string(),
            name: summary.name.clone(),
            description: summary.description.clone(),
            created_at: rfc3339(summary.created_at),
            last_updated: rfc3339(summary.last_updated),
            update_count: summary.update_count,
            entity_count,
        }
    }
}

/// A session's metadata.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    /// Its id.
    pub session_id: String,
    /// Its name ("" when none).
    pub name: String,
    /// Its description ("" when none).
    pub description: String,
    /// Creation time, Unix seconds.
    pub created_at_unix: i64,
    /// Its records.
    pub update_count: usize,
}

impl From<&SessionSummary> for SessionInfo {
    fn from(s: &SessionSummary) -> Self {
        Self {
            session_id: s.id.to_string(),
            name: s.name.clone().unwrap_or_default(),
            description: s.description.clone().unwrap_or_default(),
            created_at_unix: s.created_at.timestamp(),
            update_count: s.update_count,
        }
    }
}

/// The sessions whose name or description matches a query.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSearch {
    /// The query.
    pub query: String,
    /// The matches.
    pub sessions: Vec<SessionInfo>,
}

/// Captured messages deleted.
#[derive(Debug, Clone, Serialize)]
pub struct MessagesDeleted {
    /// The session.
    pub session_id: String,
    /// Messages deleted.
    pub deleted: u64,
    /// Messages left.
    pub remaining: u64,
}

/// A session created.
#[derive(Debug, Clone, Serialize)]
pub struct SessionCreated {
    /// Its id.
    pub session_id: String,
    /// Its name, when one was given.
    pub name: Option<String>,
    /// Its description, when one was given.
    pub description: Option<String>,
}

/// A session's name or description changed.
#[derive(Debug, Clone, Serialize)]
pub struct SessionUpdated {
    /// The session.
    pub session_id: String,
}

/// A session deleted.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDeleted {
    /// The session.
    pub session_id: String,
    /// Whether it was stored (false: nothing to delete).
    pub deleted: bool,
}
