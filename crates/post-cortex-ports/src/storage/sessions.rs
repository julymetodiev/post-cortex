// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The session port: a session, its records and its entity graph, saved
//! with the changes each save carries (ARCHITECTURE section 5).

use crate::error::Result;
use async_trait::async_trait;
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use uuid::Uuid;

use crate::storage::SessionSummary;

/// What a save of a session changes; everything else stored for it stays.
///
/// A record changes only the entities it names and the edges it states, so
/// the adapter writes those graph rows for every record in `updates`, read
/// from the snapshot being saved. Mentions are not stored: a load derives
/// them from the records.
#[derive(Debug, Clone, Default)]
pub struct SessionChanges {
    /// Record rows to write (new records and changed ones), in session
    /// order.
    pub updates: Vec<ContextUpdate>,
    /// Records whose rows are deleted. A save never infers a removal from
    /// what a snapshot lacks: the snapshot may predate a record.
    pub removed: Vec<Uuid>,
    /// The graph changed other than through `updates` (an entity deleted or
    /// merged, the graph rebuilt): the snapshot's whole graph replaces the
    /// stored one.
    pub whole_graph: bool,
    /// Records embedded since the last save.
    pub vectorized: Vec<Uuid>,
    /// Records no longer embedded since the last save.
    pub unvectorized: Vec<Uuid>,
}

impl SessionChanges {
    /// Everything of `session`: every record, the whole graph and the
    /// embedded records `vectorized` (a new or imported session).
    pub fn everything(session: &ActiveSession, vectorized: &[Uuid]) -> Self {
        Self {
            updates: session.records_owned(),
            removed: Vec::new(),
            whole_graph: true,
            vectorized: vectorized.to_vec(),
            unvectorized: Vec::new(),
        }
    }

    /// The records `updates` (new or changed).
    pub fn records(updates: Vec<ContextUpdate>) -> Self {
        Self {
            updates,
            ..Self::default()
        }
    }

    /// The records `removed` deleted.
    pub fn removals(removed: Vec<Uuid>) -> Self {
        Self {
            removed,
            ..Self::default()
        }
    }

    /// The graph changed other than through records.
    pub fn graph() -> Self {
        Self {
            whole_graph: true,
            ..Self::default()
        }
    }

    /// The records `removed` deleted and the graph rebuilt without them.
    pub fn rebuild(removed: Vec<Uuid>) -> Self {
        Self {
            removed,
            whole_graph: true,
            ..Self::default()
        }
    }

    /// Nothing changes but what [`Self::merge`] adds.
    pub fn is_empty(&self) -> bool {
        self.updates.is_empty()
            && self.removed.is_empty()
            && !self.whole_graph
            && self.vectorized.is_empty()
            && self.unvectorized.is_empty()
    }

    /// The changes of this save followed by `later` ones (of a later
    /// request for the same session): a record `later` writes or removes
    /// is taken from `later`, an id `later` (un)vectorizes likewise.
    pub fn merge(&mut self, later: SessionChanges) {
        let touched: std::collections::HashSet<Uuid> = later
            .updates
            .iter()
            .map(|u| u.id)
            .chain(later.removed.iter().copied())
            .collect();
        self.updates.retain(|u| !touched.contains(&u.id));
        self.removed.retain(|id| !touched.contains(id));
        self.updates.extend(later.updates);
        self.removed.extend(later.removed);
        self.whole_graph |= later.whole_graph;
        let flipped: std::collections::HashSet<Uuid> = later
            .vectorized
            .iter()
            .chain(&later.unvectorized)
            .copied()
            .collect();
        self.vectorized.retain(|id| !flipped.contains(id));
        self.unvectorized.retain(|id| !flipped.contains(id));
        self.vectorized.extend(later.vectorized);
        self.unvectorized.extend(later.unvectorized);
    }
}

/// Stored sessions with their records and entity graphs. Every session is
/// loaded with the ids of its embedded records (`vectorized`, the memory
/// runtime's), stored beside it.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Save `session` (its metadata) with `changes`, in one batch or
    /// transaction. Changed graph rows are read from `session`.
    async fn save_session(&self, session: &ActiveSession, changes: SessionChanges) -> Result<()>;

    /// Load a session with the ids of its embedded records; not-found
    /// ([`crate::PortError::NotFound`]) when there is none.
    async fn load_session(&self, session_id: Uuid) -> Result<StoredSession>;

    /// Delete a session and everything stored for it: records, graph,
    /// vectors, messages, source references and checkpoints. Returns
    /// whether the session was stored.
    async fn delete_session(&self, session_id: Uuid) -> Result<bool>;

    /// Every stored session id.
    async fn list_sessions(&self) -> Result<Vec<Uuid>>;

    /// Every stored session's metadata (no particular order) without
    /// loading the sessions themselves.
    async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>>;

    /// Whether the session is stored.
    async fn session_exists(&self, session_id: Uuid) -> Result<bool>;

    /// Replace a stored session (or store a new one) with `session`: the
    /// target's records, graph, vectors and messages are deleted and
    /// `session` is written, in one batch or transaction. Checkpoints and
    /// workspace memberships stay. What `import --overwrite` does to a
    /// session.
    async fn replace_session(&self, session: &ActiveSession, vectorized: &[Uuid]) -> Result<()>;
}

#[cfg(test)]
mod tests;
