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

//! Row layouts written before data version 2 (see [`crate::rocksdb_storage::rows`]), read
//! only by the migration at open ([`crate::rocksdb_storage::migrate`]).
//!
//! Bincode is not self-describing: a struct with a new field cannot read
//! rows written without it. Update rows end with the update, so a newer
//! layout fails cleanly on an older row (the input ends before the new
//! field) and [`decode_update`] falls back layout by layout: current, then
//! [`DerivedContextUpdate`], [`AuthoredContextUpdate`] and
//! [`LegacyContextUpdate`]. Checkpoints hold a list of updates followed by
//! more fields, where a misread would not fail cleanly, so each layout has
//! its marker: `pcx-ckpt-v4` for the current one (also data version 1),
//! [`CHECKPOINT_V3`] for derived, [`CHECKPOINT_V2`] for authored updates,
//! none for legacy ones.
//!
//! Workspace rows gained `projects`, then `archived_at`: rows with both
//! start with [`WORKSPACE_V3`] ([`V3StoredWorkspace`]), rows with projects
//! only with [`WORKSPACE_V2`] (read as [`ProjectsStoredWorkspace`]),
//! unmarked ones are read as [`LegacyStoredWorkspace`]; rows of data
//! versions 1 to 4 start with [`WORKSPACE_V4`] ([`V4WorkspaceRow`], before
//! the slug), rows of data version 5 with [`WORKSPACE_V5`]
//! ([`V5WorkspaceRow`], before remote reranking). Membership rows
//! were unmarked [`LegacyMembership`]s; source references are in
//! [`source`].
//!
//! These structs are frozen: they must keep the field order and types of
//! the old layout.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    CodeReference, ContextUpdate, EntityRelationship, TypedEntity, UpdateContent, UpdateType,
};
use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_core::session::active_session::ChangeRecord;
use post_cortex_core::workspace::{SessionRole, WorkspaceProject};

use crate::rocksdb_storage::rows::{CHECKPOINT_V4, MembershipRow, ProjectRow};
use post_cortex_ports::storage::SessionCheckpoint;

pub mod source;

/// Marks a checkpoint written with [`DerivedContextUpdate`]s.
const CHECKPOINT_V3: &[u8] = b"pcx-ckpt-v3\0";

/// Marks a checkpoint written with [`AuthoredContextUpdate`]s.
const CHECKPOINT_V2: &[u8] = b"pcx-ckpt-v2\0";

/// Marks a workspace row written with `projects`, before `archived_at`.
const WORKSPACE_V2: &[u8] = b"pcx-ws-v2\0";

/// Marks a workspace row with `projects` and `archived_at` and members.
const WORKSPACE_V3: &[u8] = b"pcx-ws-v3\0";

/// Marks a workspace row of data versions 1 to 4 ([`V4WorkspaceRow`]).
pub const WORKSPACE_V4: &[u8] = b"pcx-ws-v4\0";

/// Marks a workspace row of data version 5 ([`V5WorkspaceRow`]).
pub const WORKSPACE_V5: &[u8] = b"pcx-ws-v5\0";

/// The workspace row of data version 5: with its slug, before remote
/// reranking. Step 5 writes it; step 6 forbids each the remote reranker.
#[derive(serde::Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct V5WorkspaceRow {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub created_at: u64,
    pub projects: Vec<ProjectRow>,
    pub archived_at: Option<u64>,
}

/// The workspace row of data versions 1 to 4: before its slug. Step 1
/// writes older rows in it; step 5 gives each its slug.
#[derive(serde::Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct V4WorkspaceRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub created_at: u64,
    pub projects: Vec<ProjectRow>,
    pub archived_at: Option<u64>,
}

/// `StoredWorkspace` before `projects`.
#[derive(Deserialize)]
struct LegacyStoredWorkspace {
    id: Uuid,
    name: String,
    description: String,
    /// Decoded for the row layout only: members are the `ws_session:`
    /// rows, which listings read.
    _sessions: Vec<(Uuid, SessionRole)>,
    created_at: u64,
}

impl From<LegacyStoredWorkspace> for V4WorkspaceRow {
    fn from(w: LegacyStoredWorkspace) -> Self {
        Self {
            id: w.id,
            name: w.name,
            description: w.description,
            created_at: w.created_at,
            projects: Vec::new(),
            archived_at: None,
        }
    }
}

/// `StoredWorkspace` with `projects`, before `archived_at`.
#[derive(Deserialize)]
struct ProjectsStoredWorkspace(LegacyStoredWorkspace, Vec<WorkspaceProject>);

impl From<ProjectsStoredWorkspace> for V4WorkspaceRow {
    fn from(ProjectsStoredWorkspace(w, projects): ProjectsStoredWorkspace) -> Self {
        Self {
            projects: crate::rocksdb_storage::rows::WorkspaceRow::from_projects(&projects),
            ..w.into()
        }
    }
}

/// `StoredWorkspace` with `projects` and `archived_at` ([`WORKSPACE_V3`]).
#[derive(Deserialize)]
struct V3StoredWorkspace(LegacyStoredWorkspace, Vec<WorkspaceProject>, Option<u64>);

impl From<V3StoredWorkspace> for V4WorkspaceRow {
    fn from(V3StoredWorkspace(w, projects, archived_at): V3StoredWorkspace) -> Self {
        Self {
            archived_at,
            ..ProjectsStoredWorkspace(w, projects).into()
        }
    }
}

/// A membership row before markers (`StoredWorkspaceSession`).
#[derive(Deserialize)]
struct LegacyMembership {
    workspace_id: Uuid,
    session_id: Uuid,
    role: SessionRole,
    added_at: u64,
}

/// `ContextUpdate` before `author`.
#[derive(Deserialize)]
struct LegacyContextUpdate {
    id: Uuid,
    timestamp: DateTime<Utc>,
    update_type: UpdateType,
    content: UpdateContent,
    related_code: Option<CodeReference>,
    parent_update: Option<Uuid>,
    user_marked_important: bool,
    creates_entities: Vec<String>,
    creates_relationships: Vec<EntityRelationship>,
    references_entities: Vec<String>,
    typed_entities: Vec<TypedEntity>,
}

impl From<LegacyContextUpdate> for ContextUpdate {
    fn from(u: LegacyContextUpdate) -> Self {
        Self {
            id: u.id,
            timestamp: u.timestamp,
            update_type: u.update_type,
            content: u.content,
            related_code: u.related_code,
            parent_update: u.parent_update,
            user_marked_important: u.user_marked_important,
            creates_entities: u.creates_entities,
            creates_relationships: u.creates_relationships,
            references_entities: u.references_entities,
            typed_entities: u.typed_entities,
            author: None,
            derived_from: Vec::new(),
            meta: Default::default(),
        }
    }
}

/// `ContextUpdate` with `author`, before `derived_from`. Bincode writes a
/// struct as its fields in order, so this is the legacy fields followed by
/// `author`.
#[derive(Deserialize)]
struct AuthoredContextUpdate(LegacyContextUpdate, Option<String>);

impl From<AuthoredContextUpdate> for ContextUpdate {
    fn from(AuthoredContextUpdate(update, author): AuthoredContextUpdate) -> Self {
        Self {
            author,
            ..update.into()
        }
    }
}

/// `ContextUpdate` with `author` and `derived_from`, before `meta`.
#[derive(Deserialize)]
struct DerivedContextUpdate(LegacyContextUpdate, Option<String>, Vec<String>);

impl From<DerivedContextUpdate> for ContextUpdate {
    fn from(DerivedContextUpdate(update, author, derived_from): DerivedContextUpdate) -> Self {
        Self {
            author,
            derived_from,
            ..update.into()
        }
    }
}

/// `SessionCheckpoint` holding updates in an older layout `U`.
#[derive(Deserialize)]
struct LegacySessionCheckpoint<U> {
    id: Uuid,
    session_id: Uuid,
    created_at: DateTime<Utc>,
    structured_context: StructuredContext,
    recent_updates: Vec<U>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<ChangeRecord>,
    total_updates: usize,
    context_quality_score: f32,
    compression_ratio: f32,
}

impl<U: Into<ContextUpdate>> From<LegacySessionCheckpoint<U>> for SessionCheckpoint {
    fn from(c: LegacySessionCheckpoint<U>) -> Self {
        Self {
            id: c.id,
            session_id: c.session_id,
            created_at: c.created_at,
            structured_context: c.structured_context,
            recent_updates: c.recent_updates.into_iter().map(Into::into).collect(),
            code_references: c.code_references,
            change_history: c.change_history,
            total_updates: c.total_updates,
            context_quality_score: c.context_quality_score,
            compression_ratio: c.compression_ratio,
        }
    }
}

fn bincode_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    bincode::serde::decode_from_slice::<T, _>(bytes, bincode::config::standard())
        .ok()
        .map(|(v, _)| v)
}

/// An update row in the current layout, else in the layout before `meta`,
/// before `derived_from`, before `author`.
pub fn decode_update(bytes: &[u8]) -> Option<ContextUpdate> {
    bincode_decode::<ContextUpdate>(bytes)
        .or_else(|| bincode_decode::<DerivedContextUpdate>(bytes).map(Into::into))
        .or_else(|| bincode_decode::<AuthoredContextUpdate>(bytes).map(Into::into))
        .or_else(|| bincode_decode::<LegacyContextUpdate>(bytes).map(Into::into))
}

/// A checkpoint in the layout its marker names; unmarked ones are from
/// before `author`.
pub fn decode_checkpoint(bytes: &[u8]) -> Option<SessionCheckpoint> {
    if let Some(rest) = bytes.strip_prefix(CHECKPOINT_V4) {
        bincode_decode::<SessionCheckpoint>(rest)
    } else if let Some(rest) = bytes.strip_prefix(CHECKPOINT_V3) {
        bincode_decode::<LegacySessionCheckpoint<DerivedContextUpdate>>(rest).map(Into::into)
    } else if let Some(rest) = bytes.strip_prefix(CHECKPOINT_V2) {
        bincode_decode::<LegacySessionCheckpoint<AuthoredContextUpdate>>(rest).map(Into::into)
    } else {
        bincode_decode::<LegacySessionCheckpoint<LegacyContextUpdate>>(bytes).map(Into::into)
    }
}

/// A workspace row in the layout its marker names; unmarked ones are from
/// before `projects`.
pub fn decode_workspace(bytes: &[u8]) -> Option<V4WorkspaceRow> {
    if let Some(rest) = bytes.strip_prefix(WORKSPACE_V4) {
        bincode_decode::<V4WorkspaceRow>(rest)
    } else if let Some(rest) = bytes.strip_prefix(WORKSPACE_V3) {
        bincode_decode::<V3StoredWorkspace>(rest).map(Into::into)
    } else if let Some(rest) = bytes.strip_prefix(WORKSPACE_V2) {
        bincode_decode::<ProjectsStoredWorkspace>(rest).map(Into::into)
    } else {
        bincode_decode::<LegacyStoredWorkspace>(bytes).map(Into::into)
    }
}

/// An unmarked membership row.
pub fn decode_membership(bytes: &[u8]) -> Option<MembershipRow> {
    bincode_decode::<LegacyMembership>(bytes)
        .map(|m| MembershipRow::new(m.workspace_id, m.session_id, m.role, m.added_at))
}

/// A row of a family data version 1 stored unmarked (embeddings,
/// messages, cards, radar dismissals): bincode of the type itself.
pub fn decode_unmarked<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    bincode_decode(bytes)
}

#[cfg(test)]
mod tests;
