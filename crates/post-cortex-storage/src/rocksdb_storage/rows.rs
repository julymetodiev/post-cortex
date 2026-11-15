// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! On-disk row formats of the RocksDB backend (data version 3).
//!
//! Every row value starts with a marker that names its layout; a reader
//! decodes exactly the layout its marker names and anything else is
//! [`Corrupt`](post_cortex_ports::PortError::Corrupt). Rows of older layouts are
//! rewritten once, when the database is opened (see [`crate::rocksdb_storage::migrate`]);
//! nothing on the read path decodes a legacy layout.
//!
//! | key | marker | value |
//! |---|---|---|
//! | `session:{id}` | [`SESSION_V2`] | bincode [`SessionHead`] |
//! | `session:{id}:rec:{ordinal}` | [`UPDATE_V4`] | bincode `ContextUpdate` (layout 4) |
//! | `session:{id}:recid:{uid}` | none | the record's ordinal, 8 bytes big-endian |
//! | `session:{id}:next` | none | the next ordinal, 8 bytes big-endian |
//! | `session:{id}:ent:{name}` | [`ENTITY_V1`] | bincode `EntityData` |
//! | `session:{id}:edge:{from}\0{to}\0{type}` | [`EDGE_V1`] | bincode `EntityRelationship` |
//! | `session:{id}:vec:{uid}` | none | empty: the record has its embedding |
//! | `session_meta:{id}` | [`META_V1`] | JSON [`SessionSummary`] |
//! | `checkpoint:{id}` | [`CHECKPOINT_V4`] | bincode [`SessionCheckpoint`] |
//! | `workspace:{id}` | [`WORKSPACE_V6`] | bincode [`WorkspaceRow`] |
//! | `ws_session:{ws}:{session}` | [`MEMBERSHIP_V1`] | bincode [`MembershipRow`] |
//! | `embedding:{session}:{content}` | [`EMBEDDING_V1`] | bincode `StoredEmbedding` |
//! | `message:{session}:{seq}` | [`MESSAGE_V2`] ([`MESSAGE_V1`] read) | bincode `MessageRowV2` (`message_rows`) |
//! | `card:{scope}:{version}` | [`CARD_V1`] | bincode `Card` |
//! | `doc:{ws}:{id}:{version}` | [`DOCUMENT_V1`] | JSON `Document` |
//! | `doc_head:{ws}:{id}` | none | the latest version, 8 bytes big-endian |
//! | `doc_order:{ws}:{kind}:{inv}:{id}` | none | empty: the kind's latest versions, newest change first (`document_index`) |
//! | `doc_key:{ws}:{kind}:{key}:{inv}:{id}` | none | empty: the latest versions with a key |
//! | `doc_keys:{ws}:{id}` | [`DOCUMENT_KEYS_V1`] | JSON list of the latest version's keys |
//! | `doc_unkeyed:{ws}:{id}` | none | empty: the latest version has no keys |
//! | `radar_dismissal:{ws}:{kind}:{key}` | [`DISMISSAL_V1`] | bincode `RadarDismissal` |
//!
//! Update rows and checkpoints hold domain types in bincode, which is not
//! self-describing: a change to `ContextUpdate`'s fields changes the layout.
//! `rows/tests.rs` pins layout 4 against bytes written by 0.5.0; when it
//! fails, freeze the old layout in `legacy.rs`, bump the marker and add a
//! migration step. The session layout of data versions 1 and 2 (one JSON
//! blob with the graph and the vectorized ids, and an update index) is in
//! [`crate::rocksdb_storage::v2_layout`], which only the migration reads.

use crate::port_error::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::{ContextUpdate, EntityData, EntityRelationship};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::session::session_components::SessionMetadata;
use post_cortex_core::workspace::{SessionRole, WorkspaceProject};

use post_cortex_ports::error::{corrupt, parse_name};
use post_cortex_ports::storage::SessionSummary;
use post_cortex_ports::storage::{SessionCheckpoint, StoredWorkspace};

/// Session head: metadata and preferences (data version 3).
pub const SESSION_V2: &[u8] = b"pcx-ses-v2\0";
/// Entity row of a session's graph (data version 3).
pub const ENTITY_V1: &[u8] = b"pcx-ent-v1\0";
/// Edge row of a session's graph (data version 3).
pub const EDGE_V1: &[u8] = b"pcx-edge-v1\0";
/// Update row, bincode `ContextUpdate` with `author`, `derived_from`, `meta`.
pub const UPDATE_V4: &[u8] = b"pcx-upd-v4\0";
/// Listing record, JSON [`SessionSummary`].
pub const META_V1: &[u8] = b"pcx-meta-v1\0";
/// Checkpoint holding layout-4 updates.
pub const CHECKPOINT_V4: &[u8] = b"pcx-ckpt-v4\0";
/// Workspace row without members (they are `ws_session:` rows), with its
/// slug and remote reranking (data version 6).
pub const WORKSPACE_V6: &[u8] = b"pcx-ws-v6\0";
/// Workspace membership row.
pub const MEMBERSHIP_V1: &[u8] = b"pcx-wsm-v1\0";
/// Embedding row (data version 2).
pub const EMBEDDING_V1: &[u8] = b"pcx-emb-v1\0";
/// Captured message row (data version 2).
pub const MESSAGE_V1: &[u8] = b"pcx-msg-v1\0";
/// Message row with its parent and blocks (W18).
pub const MESSAGE_V2: &[u8] = b"pcx-msg-v2\0";
/// Card version row (data version 2).
pub const CARD_V1: &[u8] = b"pcx-card-v1\0";
/// Radar dismissal row (data version 2).
pub const DISMISSAL_V1: &[u8] = b"pcx-dis-v1\0";
/// Document version row (JSON: its data is free-form).
pub const DOCUMENT_V1: &[u8] = b"pcx-doc-v1\0";
/// The keys of a document's latest version (JSON list of their text).
pub const DOCUMENT_KEYS_V1: &[u8] = b"pcx-dky-v1\0";
/// Bus event row (docs/host.md, events of a module).
pub const EVENT_V1: &[u8] = b"pcx-evt-v1\0";

/// A workspace as stored: everything but its members.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    /// Unix seconds; set when the workspace is created, kept by every save.
    pub created_at: u64,
    pub projects: Vec<ProjectRow>,
    pub archived_at: Option<u64>,
    /// Whether its accurate searches may use the remote reranker.
    pub remote_rerank: bool,
}

/// A project of a workspace row.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub key: String,
    pub session_id: Uuid,
}

impl WorkspaceRow {
    pub fn from_projects(projects: &[WorkspaceProject]) -> Vec<ProjectRow> {
        projects
            .iter()
            .map(|p| ProjectRow {
                key: p.key.clone(),
                session_id: p.session_id,
            })
            .collect()
    }

    /// The listing form, with `members`.
    pub fn into_stored(self, members: Vec<(Uuid, SessionRole)>) -> StoredWorkspace {
        StoredWorkspace {
            id: self.id,
            name: self.name,
            slug: self.slug,
            description: self.description,
            sessions: members,
            created_at: self.created_at,
            projects: self
                .projects
                .into_iter()
                .map(|p| WorkspaceProject {
                    key: p.key,
                    session_id: p.session_id,
                })
                .collect(),
            archived_at: self.archived_at,
            remote_rerank: self.remote_rerank,
        }
    }
}

/// A session's membership of a workspace. The role is its wire name.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct MembershipRow {
    pub workspace_id: Uuid,
    pub session_id: Uuid,
    pub role: String,
    pub added_at: u64,
}

impl MembershipRow {
    pub fn new(workspace_id: Uuid, session_id: Uuid, role: SessionRole, at: u64) -> Self {
        Self {
            workspace_id,
            session_id,
            role: role.as_str().to_string(),
            added_at: at,
        }
    }

    pub fn role(&self) -> Result<SessionRole> {
        Ok(parse_name(
            "workspace membership",
            self.session_id,
            &self.role,
        )?)
    }
}

/// `value` as JSON after `marker`, written into one buffer.
pub fn json_marked<T: Serialize>(marker: &[u8], value: &T) -> Result<Vec<u8>> {
    let mut row = marker.to_vec();
    serde_json::to_writer(&mut row, value)?;
    Ok(row)
}

/// The JSON `T` after `marker`, or `Corrupt` naming `kind` and `key`.
pub fn decode_json_marked<T: DeserializeOwned>(
    kind: &'static str,
    key: &str,
    marker: &[u8],
    row: &[u8],
) -> Result<T> {
    let json = body(kind, key, marker, row)?;
    serde_json::from_slice(json).map_err(|e| corrupt(kind, key, e).into())
}

/// `value` as bincode after `marker`, written into one buffer.
fn bincode_marked<T: Serialize>(marker: &[u8], value: &T) -> Result<Vec<u8>> {
    let mut row = marker.to_vec();
    bincode::serde::encode_into_std_write(value, &mut row, bincode::config::standard())?;
    Ok(row)
}

/// The body after `marker`, or `Corrupt` naming `kind` and `key`.
pub fn body<'a>(kind: &'static str, key: &str, marker: &[u8], row: &'a [u8]) -> Result<&'a [u8]> {
    row.strip_prefix(marker)
        .ok_or_else(|| corrupt(kind, key, "unknown row layout").into())
}

/// Bincode of exactly `T` (trailing bytes are corruption too).
fn bincode_exact<T: DeserializeOwned>(kind: &'static str, key: &str, bytes: &[u8]) -> Result<T> {
    let (value, read) =
        bincode::serde::decode_from_slice::<T, _>(bytes, bincode::config::standard())
            .map_err(|e| corrupt(kind, key, e))?;
    if read != bytes.len() {
        return Err(corrupt(kind, key, "trailing bytes").into());
    }
    Ok(value)
}

/// What a session row holds besides its records and graph.
#[derive(Serialize, Deserialize)]
pub struct SessionHead {
    pub metadata: SessionMetadata,
    pub last_updated: DateTime<Utc>,
}

impl SessionHead {
    pub fn of(session: &ActiveSession) -> Self {
        Self {
            metadata: SessionMetadata::clone(session.metadata()),
            last_updated: session.last_updated(),
        }
    }
}

pub fn encode_head(session: &ActiveSession) -> Result<Vec<u8>> {
    bincode_marked(SESSION_V2, &SessionHead::of(session))
}

pub fn decode_head(key: &str, row: &[u8]) -> Result<SessionHead> {
    bincode_exact("session", key, body("session", key, SESSION_V2, row)?)
}

pub fn encode_entity(entity: &EntityData) -> Result<Vec<u8>> {
    bincode_marked(ENTITY_V1, entity)
}

pub fn decode_entity(key: &str, row: &[u8]) -> Result<EntityData> {
    bincode_exact("entity", key, body("entity", key, ENTITY_V1, row)?)
}

pub fn encode_edge(edge: &EntityRelationship) -> Result<Vec<u8>> {
    bincode_marked(EDGE_V1, edge)
}

pub fn decode_edge(key: &str, row: &[u8]) -> Result<EntityRelationship> {
    bincode_exact("edge", key, body("edge", key, EDGE_V1, row)?)
}

pub fn encode_update(update: &ContextUpdate) -> Result<Vec<u8>> {
    bincode_marked(UPDATE_V4, update)
}

pub fn decode_update(key: &str, row: &[u8]) -> Result<ContextUpdate> {
    bincode_exact("update row", key, body("update row", key, UPDATE_V4, row)?)
}

pub fn encode_meta(summary: &SessionSummary) -> Result<Vec<u8>> {
    json_marked(META_V1, summary)
}

pub fn decode_meta(key: &str, row: &[u8]) -> Result<SessionSummary> {
    decode_json_marked("session listing", key, META_V1, row)
}

pub fn encode_checkpoint(checkpoint: &SessionCheckpoint) -> Result<Vec<u8>> {
    bincode_marked(CHECKPOINT_V4, checkpoint)
}

pub fn decode_checkpoint(key: &str, row: &[u8]) -> Result<SessionCheckpoint> {
    bincode_exact(
        "checkpoint",
        key,
        body("checkpoint", key, CHECKPOINT_V4, row)?,
    )
}

pub fn encode_workspace(workspace: &WorkspaceRow) -> Result<Vec<u8>> {
    bincode_marked(WORKSPACE_V6, workspace)
}

pub fn decode_workspace(key: &str, row: &[u8]) -> Result<WorkspaceRow> {
    bincode_exact("workspace", key, body("workspace", key, WORKSPACE_V6, row)?)
}

pub fn encode_membership(membership: &MembershipRow) -> Result<Vec<u8>> {
    bincode_marked(MEMBERSHIP_V1, membership)
}

pub fn decode_membership(key: &str, row: &[u8]) -> Result<MembershipRow> {
    let kind = "workspace membership";
    bincode_exact(kind, key, body(kind, key, MEMBERSHIP_V1, row)?)
}

/// `value` as bincode after `marker`.
pub fn encode_marked<T: Serialize>(marker: &[u8], value: &T) -> Result<Vec<u8>> {
    bincode_marked(marker, value)
}

/// The bincode `T` after `marker`, or `Corrupt` naming `kind` and `key`.
pub fn decode_marked<T: DeserializeOwned>(
    kind: &'static str,
    key: &str,
    marker: &[u8],
    row: &[u8],
) -> Result<T> {
    bincode_exact(kind, key, body(kind, key, marker, row)?)
}

#[cfg(test)]
mod tests;
