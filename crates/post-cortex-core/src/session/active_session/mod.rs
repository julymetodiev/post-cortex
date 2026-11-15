// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A session as a value: its metadata, its records (append-only, oldest
//! first) and its entity graph. Everything else (current state, flow, hot
//! window, code references, change history) is derived from the records on
//! read ([`views`]); the record list changes only through
//! [`ActiveSession::apply_record`], [`ActiveSession::remove_records`] and
//! [`ActiveSession::retire_update`] ([`mutate`]). Time is always a
//! parameter.
//!
//! The value holds its components behind `Arc`s so that a copy (one per
//! published version in the memory crate's session cache) shares them:
//! a write copies the record handles, not the records.

mod codec;
mod mutate;
mod views;

#[cfg(test)]
mod tests;

pub use codec::{ActiveSessionView, StoredSession};
pub use views::ChangeRecord;

use crate::collections::SharedVec;
use crate::core::context_update::ContextUpdate;
use crate::graph::entity_graph::SimpleEntityGraph;
use crate::session::session_components::SessionMetadata;

use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

/// A session: metadata, records and entity graph (see the module docs).
#[derive(Clone, Debug)]
pub struct ActiveSession {
    metadata: Arc<SessionMetadata>,
    last_updated: DateTime<Utc>,
    records: SharedVec<Arc<ContextUpdate>>,
    entity_graph: Arc<SimpleEntityGraph>,
}

/// User-configurable session preferences.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserPreferences {
    /// Whether automatic saving is enabled
    pub auto_save_enabled: bool,
    /// How many days to retain context before pruning
    pub context_retention_days: u32,
    /// How many of the newest records the hot window holds
    pub max_hot_context_size: usize,
    /// Kept for the stored format; periodic summaries are no longer made
    pub auto_summary_threshold: usize,
    /// Keywords the user considers important for prioritisation
    pub important_keywords: Vec<String>,
}

/// The one default for sessions and for stored sessions without
/// preferences.
impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            auto_save_enabled: true,
            context_retention_days: 30,
            max_hot_context_size: 50,
            auto_summary_threshold: 100,
            important_keywords: vec![],
        }
    }
}

/// Largest update description kept, in bytes; longer ones are cut. Long
/// enough for whole answers, code and logs: search embeds a summary of long
/// records and returns them in full.
pub const MAX_DESCRIPTION_BYTES: usize = 64 * 1024;

/// Longest title kept, in bytes; longer ones are cut to
/// `TRUNCATED_TITLE_BYTES` plus an ellipsis.
pub const MAX_TITLE_BYTES: usize = 200;
const TRUNCATED_TITLE_BYTES: usize = 190;

/// Truncate a string to at most `max_bytes` without splitting a multi-byte
/// UTF-8 character.
fn truncate_safe(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

/// `update` within the size limits: a description over
/// [`MAX_DESCRIPTION_BYTES`] and a title over [`MAX_TITLE_BYTES`] are cut
/// at a character boundary and marked as cut.
pub fn normalised(mut update: ContextUpdate) -> ContextUpdate {
    if update.content.description.len() > MAX_DESCRIPTION_BYTES {
        truncate_safe(&mut update.content.description, MAX_DESCRIPTION_BYTES);
        update.content.description.push_str("... (truncated)");
    }
    if update.content.title.len() > MAX_TITLE_BYTES {
        truncate_safe(&mut update.content.title, TRUNCATED_TITLE_BYTES);
        update.content.title.push_str("...");
    }
    update
}

impl ActiveSession {
    /// A new empty session created at `created_at`, with default
    /// preferences.
    pub fn new(
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        let metadata = SessionMetadata::new(
            id,
            name,
            description,
            UserPreferences::default(),
            created_at,
        );
        Self::from_records(metadata, created_at, Vec::new(), SimpleEntityGraph::new())
    }

    /// A session from what storage keeps: metadata, records (oldest
    /// first) and the entity graph. The graph's
    /// record links are derived from the records, so storage need not keep
    /// them. The one constructor every backend uses.
    pub fn from_records(
        metadata: SessionMetadata,
        last_updated: DateTime<Utc>,
        records: Vec<ContextUpdate>,
        entity_graph: SimpleEntityGraph,
    ) -> Self {
        let mut session = Self {
            metadata: Arc::new(metadata),
            last_updated,
            records: SharedVec::new(),
            entity_graph: Arc::new(entity_graph),
        };
        session.replace_records(records);
        session
    }

    /// The session's unique identifier.
    pub fn id(&self) -> Uuid {
        self.metadata.id
    }

    /// The session's metadata (name, description, creation time,
    /// preferences).
    pub fn metadata(&self) -> &SessionMetadata {
        &self.metadata
    }

    /// The session's display name, if set.
    pub fn name(&self) -> Option<String> {
        self.metadata.name.clone()
    }

    /// The session's description, if set.
    pub fn description(&self) -> Option<String> {
        self.metadata.description.clone()
    }

    /// Name and description.
    pub fn get_metadata(&self) -> (Option<String>, Option<String>) {
        (
            self.metadata.name.clone(),
            self.metadata.description.clone(),
        )
    }

    /// When the session was created.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.metadata.created_at
    }

    /// When the session last changed.
    pub fn last_updated(&self) -> DateTime<Utc> {
        self.last_updated
    }

    /// The session's preferences.
    pub fn user_preferences(&self) -> &UserPreferences {
        &self.metadata.user_preferences
    }

    /// Every record, oldest first. Shared with the session's other
    /// versions; caches keyed by version compare it with
    /// [`SharedVec::ptr_eq`].
    pub fn records(&self) -> &SharedVec<Arc<ContextUpdate>> {
        &self.records
    }

    /// Owned copies of every record (for checkpoints and exports).
    pub fn records_owned(&self) -> Vec<ContextUpdate> {
        self.records
            .iter()
            .map(|u| ContextUpdate::clone(u))
            .collect()
    }

    /// The record with id `id`, if the session has it.
    pub fn record(&self, id: &Uuid) -> Option<&Arc<ContextUpdate>> {
        self.records.iter().rev().find(|u| u.id == *id)
    }

    /// The entity graph.
    pub fn entity_graph(&self) -> &SimpleEntityGraph {
        &self.entity_graph
    }

    /// The entity graph as the shared handle this version holds (callers
    /// that cache by version compare it with `Arc::ptr_eq`).
    pub fn entity_graph_shared(&self) -> &Arc<SimpleEntityGraph> {
        &self.entity_graph
    }

    /// The entity graph, for its own operations (deleting or merging
    /// entities). Copy-on-write: other versions keep theirs.
    pub fn entity_graph_mut(&mut self) -> &mut SimpleEntityGraph {
        Arc::make_mut(&mut self.entity_graph)
    }

    /// Change name and/or description (`None` keeps the current one) at
    /// `at`; the creation time is kept.
    pub fn update_metadata(
        &mut self,
        name: Option<String>,
        description: Option<String>,
        at: DateTime<Utc>,
    ) {
        let current = &self.metadata;
        self.metadata = Arc::new(SessionMetadata::new(
            current.id,
            name.or_else(|| current.name.clone()),
            description.or_else(|| current.description.clone()),
            current.user_preferences.clone(),
            current.created_at,
        ));
        self.touch(at);
    }

    /// Set the name at `at` (`None` clears it).
    pub fn set_name(&mut self, name: Option<String>, at: DateTime<Utc>) {
        let current = &self.metadata;
        self.metadata = Arc::new(SessionMetadata::new(
            current.id,
            name,
            current.description.clone(),
            current.user_preferences.clone(),
            current.created_at,
        ));
        self.touch(at);
    }

    fn touch(&mut self, at: DateTime<Utc>) {
        self.last_updated = self.last_updated.max(at);
    }
}
