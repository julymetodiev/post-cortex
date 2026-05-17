// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Request / response types for [`super::PostCortexService`].
//!
//! Kept deliberately small and self-contained — each request is a Rust
//! struct that carries exactly what the operation needs, with no
//! transport-specific fields. Transports translate their wire payload
//! (proto, MCP JSON, REST) into these types at the boundary and back
//! out at the response.
//!
//! Where a field needs to carry rich variant data (e.g. session-action
//! variants for [`ManageSessionRequest`]), it uses a Rust enum rather
//! than a bag of optional fields. This is the single biggest readability
//! win over working with the proto types directly.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::context_update::{
    CodeReference, EntityData, EntityRelationship, UpdateContent, UpdateType,
};

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// Snapshot of system liveness + capacity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Overall status — "ok", "degraded", or "down".
    pub status: String,
    /// Active session count.
    pub active_sessions: usize,
    /// Hot/warm/cold memory usage in bytes (approximate).
    pub memory_usage_bytes: u64,
    /// Number of pending items across pipeline queues.
    pub pipeline_backlog: usize,
    /// Uptime since process start, in seconds.
    pub uptime_seconds: u64,
}

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
    /// Optional structured code reference (file path + line range + snippet
    /// + git metadata). Transports translate their wire format to this
    /// shape so no transport loses fidelity.
    pub code_reference: Option<CodeReference>,
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
    /// True once the storage backend acknowledged the write. Embedding
    /// + HNSW + graph + summary updates run asynchronously and are not
    /// signalled here (per TODO.md:136-145 non-blocking writes).
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

// ---------------------------------------------------------------------------
// Read path
// ---------------------------------------------------------------------------

/// Scope of a semantic search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchScope {
    /// Search a single session by id.
    Session(Uuid),
    /// Search every session in a workspace.
    Workspace(Uuid),
    /// Search the full global index.
    Global,
}

/// Parameters for a semantic similarity search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchRequest {
    /// Natural-language query to embed and search against.
    pub query: String,
    /// Constrains which sessions are searched.
    pub scope: SearchScope,
    /// Maximum number of hits to return.
    pub limit: Option<usize>,
    /// Optional (start, end) timestamp filter.
    pub date_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// Temporal decay factor (0.0 = disabled, higher = more recent
    /// content preferred). See TODO.md scoring helpers.
    pub recency_bias: Option<f32>,
}

/// Result of a semantic search operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchResponse {
    /// Raw hits (top-K). Transport adapters often re-shape these.
    pub hits: Vec<SearchHit>,
    /// Wall-clock time the search took in milliseconds.
    pub took_ms: u64,
    /// Whether a cached result was served.
    pub used_cache: bool,
}

/// A single match from a semantic search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// Storage-level identifier of the matched content.
    pub content_id: String,
    /// Session the hit belongs to.
    pub session_id: Uuid,
    /// Type discriminator (e.g. "decision", "problem", "code_change").
    pub content_type: String,
    /// Text excerpt that matched.
    pub text: String,
    /// Raw cosine-similarity score.
    pub similarity: f32,
    /// Entity importance weight.
    pub importance: f32,
    /// Blended relevance score combining similarity, importance, and recency.
    pub combined_score: f32,
    /// When the matched content was originally written.
    pub timestamp: DateTime<Utc>,
}

/// Structured / keyword query against the session's context updates.
/// The `query_type` enum mirrors the legacy MCP `query_conversation_context`
/// `query_type` parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryContextRequest {
    /// Session to query against.
    pub session_id: Uuid,
    /// Discriminator that selects the query strategy.
    pub query_type: String,
    /// Key-value parameters forwarded to the query handler.
    pub parameters: HashMap<String, String>,
}

/// Result of a structured context query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryContextResponse {
    /// JSON-shaped result; the query_type determines the schema. Carried
    /// as serde_json::Value so transports re-serialise lossless.
    pub data: serde_json::Value,
}

/// Graph-aware retrieval — semantic search + neighbourhood traversal +
/// impact analysis composed into a single payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembleContextRequest {
    /// Natural-language query driving retrieval.
    pub query: String,
    /// Constrains which sessions are searched.
    pub scope: SearchScope,
    /// Soft cap on the total token count of the assembled context.
    pub max_tokens: Option<usize>,
    /// Whether to include impact analysis in the result.
    pub include_impact: bool,
}

/// Result of a graph-aware context assembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembleContextResponse {
    /// Human-readable, pre-formatted context text ready for LLM injection.
    pub formatted_text: String,
    /// Approximate token count of the assembled context.
    pub total_tokens: usize,
    /// Entities surfaced by graph traversal with their context metadata.
    pub entity_context: Vec<EntityContextItem>,
    /// Individual search hits that contributed to the assembled context.
    pub items: Vec<SearchHit>,
}

/// Metadata for a single entity surfaced during context assembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityContextItem {
    /// Human-readable name of the entity.
    pub entity_name: String,
    /// Type discriminator (e.g. "concept", "file", "requirement").
    pub entity_type: String,
    /// Computed importance score for the entity.
    pub importance: f32,
    /// Number of times the entity was mentioned across updates.
    pub mentions: u32,
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

/// Operations that can be performed on sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionAction {
    /// Create a new session.
    Create {
        /// Optional human-readable session name.
        name: Option<String>,
        /// Optional longer description of the session's purpose.
        description: Option<String>,
    },
    /// List all known sessions.
    List,
    /// Load a previously created session by id.
    Load {
        /// Identifier of the session to load.
        session_id: Uuid,
    },
    /// Full-text search across session metadata.
    Search {
        /// Search query string.
        query: String,
    },
    /// Update mutable metadata on an existing session.
    UpdateMetadata {
        /// Session to update.
        session_id: Uuid,
        /// New name, if changing.
        name: Option<String>,
        /// New description, if changing.
        description: Option<String>,
    },
    /// Delete a session and all associated data.
    Delete {
        /// Session to delete.
        session_id: Uuid,
    },
    /// Snapshot a session into a restorable checkpoint.
    CreateCheckpoint {
        /// Session to checkpoint.
        session_id: Uuid,
    },
    /// Restore a session from a previously saved checkpoint.
    LoadCheckpoint {
        /// Identifier of the checkpoint to restore.
        checkpoint_id: String,
        /// Session to restore the checkpoint into.
        session_id: Uuid,
    },
}

/// Request to perform a session-management action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageSessionRequest {
    /// The session action to execute.
    pub action: SessionAction,
}

/// Result of a session-management operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageSessionResponse {
    /// JSON payload whose shape depends on the executed action.
    pub data: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

/// Operations that can be performed on workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceAction {
    /// Create a new workspace.
    Create {
        /// Human-readable workspace name.
        name: String,
        /// Longer description of the workspace's purpose.
        description: String,
        /// Initial set of sessions to link to the workspace.
        session_ids: Vec<Uuid>,
    },
    /// List all known workspaces.
    List,
    /// Retrieve a single workspace by id.
    Get {
        /// Identifier of the workspace to fetch.
        workspace_id: Uuid,
    },
    /// Delete a workspace (does not delete its sessions).
    Delete {
        /// Workspace to delete.
        workspace_id: Uuid,
    },
    /// Link an existing session to a workspace.
    AddSession {
        /// Workspace to add the session to.
        workspace_id: Uuid,
        /// Session to link.
        session_id: Uuid,
        /// Optional role label for the session within this workspace.
        role: Option<String>,
    },
    /// Unlink a session from a workspace.
    RemoveSession {
        /// Workspace to remove the session from.
        workspace_id: Uuid,
        /// Session to unlink.
        session_id: Uuid,
    },
}

/// Request to perform a workspace-management action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageWorkspaceRequest {
    /// The workspace action to execute.
    pub action: WorkspaceAction,
}

/// Result of a workspace-management operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageWorkspaceResponse {
    /// JSON payload whose shape depends on the executed action.
    pub data: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Entity maintenance
// ---------------------------------------------------------------------------

/// Operations that can be performed on entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntityAction {
    /// Delete an entity and cascade through the entity graph.
    Delete {
        /// Session that owns the entity.
        session_id: Uuid,
        /// Name of the entity to delete.
        entity_name: String,
    },
    /// Delete a single context-update entry (removes from caches +
    /// persistent storage).
    DeleteUpdate {
        /// Session that owns the update.
        session_id: Uuid,
        /// Identifier of the entry to remove.
        entry_id: Uuid,
    },
}

/// Request to perform an entity-maintenance action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageEntityRequest {
    /// The entity action to execute.
    pub action: EntityAction,
}

/// Result of an entity-maintenance operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageEntityResponse {
    /// Whether the operation succeeded.
    pub success: bool,
    /// Human-readable status or error message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Analytics
// ---------------------------------------------------------------------------

/// Request for a structured summary of a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSummaryRequest {
    /// Session to summarize.
    pub session_id: Uuid,
    /// If true, return a compact projection omitting verbose fields.
    pub compact: bool,
}

/// Structured summary of a session's accumulated context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSummaryResponse {
    /// Compact JSON projection of the session — decisions, problems,
    /// entities, code references. Schema is the
    /// `summary::StructuredSummaryView` shape from
    /// `post-cortex-core::summary`.
    pub view: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

/// Administrative operations exposed through the service surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdminAction {
    /// Run the embedding pipeline over every update in a session that
    /// hasn't been vectorised yet.
    VectorizeSession {
        /// Session to vectorize.
        session_id: Uuid,
    },
    /// Snapshot a session to a serialisable checkpoint.
    CreateCheckpoint {
        /// Session to checkpoint.
        session_id: Uuid,
    },
    /// Fetch vectorisation stats (total / vectorised / pending counts).
    VectorizationStats,
    /// Health probe; mirrors `PostCortexService::health` but routed
    /// through the admin surface so MCP/REST clients can call it
    /// uniformly with the rest of the admin tools.
    Health,
}

/// Request to perform an admin action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminRequest {
    /// The admin action to execute.
    pub action: AdminAction,
}

/// Result of an admin operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminResponse {
    /// Whether the operation succeeded.
    pub success: bool,
    /// Human-readable status or error message.
    pub message: String,
    /// JSON payload whose shape depends on the executed action.
    pub data: serde_json::Value,
}
