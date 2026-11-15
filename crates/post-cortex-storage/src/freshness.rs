// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The freshness state machine of a source reference, shared by every
//! backend so they answer the same way:
//!
//! - `register_source` puts a reference in [`SourceStatus::Registered`]
//!   (whatever it was before) and records the session that registered it.
//! - `invalidate_source` (scoped to a session, or explicitly to all
//!   sessions) and `cascade_invalidate` move matching references to
//!   [`SourceStatus::Stale`]. A stale reference is kept, never deleted.
//! - A registered reference is `Fresh` while the client's hash matches the
//!   stored one (the AST hash when both sides have one), `Stale` otherwise;
//!   a stale reference is always `Stale`; an unknown entry is `Unknown`.
//! - A reference still stale after the client registered a file's symbols
//!   again is an orphan (the symbol left the file):
//!   `get_stale_entries_by_source` lists them.

use post_cortex_core::core::freshness::{FreshnessEntry, FreshnessStatus};
use uuid::Uuid;

use post_cortex_ports::storage::InvalidationScope;

/// Stored state of a source reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    /// Registered and not invalidated since.
    Registered,
    /// Invalidated and not registered again since.
    Stale,
}

impl SourceStatus {
    /// Stored code: 0 registered, 1 stale.
    pub fn code(self) -> u8 {
        match self {
            Self::Registered => 0,
            Self::Stale => 1,
        }
    }

    /// The status of a stored code; an unknown code is
    /// [`Corrupt`](post_cortex_ports::PortError::Corrupt) for reference `entry_id`.
    pub fn from_code(code: i64, entry_id: &str) -> crate::port_error::Result<Self> {
        match code {
            0 => Ok(Self::Registered),
            1 => Ok(Self::Stale),
            _ => Err(post_cortex_ports::error::corrupt(
                "source reference",
                entry_id,
                format!("status code {code}"),
            )
            .into()),
        }
    }
}

/// Whether an invalidation in `scope` reaches a reference registered by
/// `owner`. A reference stored before references recorded their session
/// (`owner == None`) belongs to no known session, so every scope reaches it.
pub fn in_scope(scope: InvalidationScope, owner: Option<Uuid>) -> bool {
    match (scope, owner) {
        (InvalidationScope::AllSessions, _) | (_, None) => true,
        (InvalidationScope::Session(session), Some(owner)) => session == owner,
    }
}

/// A stored reference as the state machine needs it.
#[derive(Debug, Clone, Copy)]
pub struct StoredState<'a> {
    /// File the reference points into.
    pub file_path: &'a str,
    /// Stored file-level hash.
    pub content_hash: &'a [u8],
    /// Stored AST hash of a function scope, when it has one.
    pub ast_hash: Option<&'a [u8]>,
    /// Stored status.
    pub status: SourceStatus,
}

/// The freshness of `entry_id` given its stored state (`None` when the entry
/// has no reference) and the client's current hashes.
pub fn evaluate(
    entry_id: String,
    stored: Option<StoredState<'_>>,
    file_hash: Vec<u8>,
    ast_hash: Option<&[u8]>,
) -> FreshnessEntry {
    let Some(stored) = stored else {
        return FreshnessEntry {
            entry_id,
            file_path: String::new(),
            status: FreshnessStatus::Unknown,
            stored_hash: Vec::new(),
            current_hash: file_hash,
        };
    };
    let fresh = match (stored.status, ast_hash, stored.ast_hash) {
        (SourceStatus::Stale, _, _) => false,
        (SourceStatus::Registered, Some(client), Some(stored_ast)) => stored_ast == client,
        (SourceStatus::Registered, _, _) => stored.content_hash == file_hash.as_slice(),
    };
    FreshnessEntry {
        entry_id,
        file_path: stored.file_path.to_string(),
        status: if fresh {
            FreshnessStatus::Fresh
        } else {
            FreshnessStatus::Stale
        },
        stored_hash: stored.content_hash.to_vec(),
        current_hash: file_hash,
    }
}
