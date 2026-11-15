// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Request and response DTOs of the memory capability services (the
//! services themselves are `post_cortex_memory::capabilities`).
//!
//! Each request carries exactly what its operation needs, with no
//! transport-specific fields; transports translate their wire payload
//! (proto, MCP JSON, REST) into these types at the boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::context_update::{
    CodeReference, EntityData, EntityRelationship, UpdateContent, UpdateType,
};

// ---------------------------------------------------------------------------
// Write path
// ---------------------------------------------------------------------------

/// A single context-update write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateContextRequest {
    /// Session this update belongs to.
    pub session_id: Uuid,
    /// Classification of the interaction (e.g. decision, problem, code change).
    pub interaction_type: UpdateType,
    /// The main body of the update.
    pub content: UpdateContent,
    /// Named entities extracted from this update.
    pub entities: Vec<EntityData>,
    /// Relationships between entities in this update.
    pub relations: Vec<EntityRelationship>,
    /// Optional structured code reference (file path, line range, snippet
    /// and git metadata). Transports translate their wire format to this
    /// shape so no transport loses fidelity.
    pub code_reference: Option<CodeReference>,
    /// Who writes it (agent name, user or teammate); transports fill in the
    /// client's name when the caller gives none.
    #[serde(default)]
    pub author: Option<String>,
    /// Captured messages of the same session the update was drawn from
    /// (`message:<seq>`); each must exist.
    #[serde(default)]
    pub derived_from: Vec<String>,
    /// Validity and trust the writer states: `valid_from`, `supersedes`
    /// (entry ids of the session), `confidence`. `retired_at` is rejected.
    #[serde(default)]
    pub meta: crate::core::context_update::UpdateMeta,
}

/// Outcome of a single write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateContextResponse {
    /// Unique identifier assigned to the persisted entry.
    pub entry_id: Uuid,
    /// Session the entry was stored under.
    pub session_id: Uuid,
    /// Timestamp when the entry was durably written.
    pub persisted_at: DateTime<Utc>,
    /// True once the storage backend acknowledged the write. Embedding,
    /// index, graph and summary updates run asynchronously and are not
    /// signalled here.
    pub durable: bool,
}

/// Batch write — N entries written under a single backend transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkUpdateContextRequest {
    /// Session all updates belong to.
    pub session_id: Uuid,
    /// Individual updates to persist atomically.
    pub updates: Vec<UpdateContextRequest>,
}

/// Outcome of a batch write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkUpdateContextResponse {
    /// Identifiers of all persisted entries.
    pub entry_ids: Vec<Uuid>,
    /// Timestamp when the batch was durably written.
    pub persisted_at: DateTime<Utc>,
    /// Whether the storage backend acknowledged the full batch.
    pub durable: bool,
}

/// Captured conversation turns for a session's message stream
/// ([`crate::core::message`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendMessagesRequest {
    /// Session whose stream receives the turns.
    pub session_id: Uuid,
    /// Turns in conversation order.
    pub messages: Vec<crate::core::message::NewMessage>,
}

/// Outcome of an append.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendMessagesResponse {
    /// Positions of the turns stored by this call, in order.
    pub appended: Vec<u64>,
    /// Turns not stored: already captured, repeated in the batch, or empty.
    pub skipped: usize,
    /// Vectors written by the call itself: always 0, since the stored turns
    /// are embedded in the background (see `queued`). Kept for clients that
    /// read it.
    pub vectorized: usize,
    /// Stored turns whose embedding was queued; 0 when embeddings are off.
    #[serde(default)]
    pub queued: usize,
}

/// Read a project or user card ([`crate::core::card`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCardRequest {
    /// Whose card.
    pub scope: crate::core::card::CardScope,
    /// Older versions to return as well (0: the latest only).
    pub history: usize,
}

/// A card and, if asked for, its older versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCardResponse {
    /// The latest version, if the card exists.
    pub card: Option<crate::core::card::Card>,
    /// Older versions, newest first.
    pub history: Vec<crate::core::card::Card>,
}

/// Write the next version of a card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCardRequest {
    /// Whose card.
    pub scope: crate::core::card::CardScope,
    /// The new markdown text (at most `MAX_CARD_BYTES`; empty clears it).
    pub text: String,
    /// The version the text was edited from (0: no card yet). The write
    /// fails if the card has changed since.
    pub expected_version: u64,
    /// Who writes it.
    pub author: Option<String>,
}

/// Outcome of a card write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCardResponse {
    /// Whether the text was stored. False: the card changed since
    /// `expected_version`; `card` is then its latest version, to merge with.
    pub written: bool,
    /// The new version, or the latest one on a conflict (`None`: no card).
    pub card: Option<crate::core::card::Card>,
}

/// Find or create the workspace and session of a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveProjectRequest {
    /// Project key computed by the client (see
    /// [`crate::workspace::WorkspaceProject::key`]).
    pub key: String,
    /// Display name of a new workspace and session.
    pub name: String,
    /// Name of the workspace to put a new project in (created if missing).
    pub workspace: Option<String>,
    /// The client asking, for the logs.
    pub client: Option<String>,
}

/// The workspace and session of a project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolveProjectResponse {
    /// Workspace the key is bound to.
    pub workspace_id: Uuid,
    /// Session of the project in that workspace.
    pub session_id: Uuid,
    /// Whether this call bound the key (created the workspace or attached
    /// the key to an existing one).
    pub created: bool,
}
