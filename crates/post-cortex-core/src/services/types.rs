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

use crate::core::context_update::{EntityData, EntityRelationship, UpdateContent, UpdateType};

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
    pub session_id: Uuid,
    pub interaction_type: UpdateType,
    pub content: UpdateContent,
    pub entities: Vec<EntityData>,
    pub relations: Vec<EntityRelationship>,
    /// Optional human-readable code reference (file:line).
    pub code_reference: Option<String>,
}

/// Outcome of a single write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateContextResponse {
    pub entry_id: Uuid,
    pub session_id: Uuid,
    pub persisted_at: DateTime<Utc>,
    /// True once the storage backend acknowledged the write. Embedding
    /// + HNSW + graph + summary updates run asynchronously and are not
    /// signalled here (per TODO.md:136-145 non-blocking writes).
    pub durable: bool,
}

/// Batch write — N entries written under a single backend transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkUpdateContextRequest {
    pub session_id: Uuid,
    pub updates: Vec<UpdateContextRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkUpdateContextResponse {
    pub entry_ids: Vec<Uuid>,
    pub persisted_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchRequest {
    pub query: String,
    pub scope: SearchScope,
    pub limit: Option<usize>,
    pub date_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// Temporal decay factor (0.0 = disabled, higher = more recent
    /// content preferred). See TODO.md scoring helpers.
    pub recency_bias: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchResponse {
    /// Raw hits (top-K). Transport adapters often re-shape these.
    pub hits: Vec<SearchHit>,
    pub took_ms: u64,
    pub used_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub content_id: String,
    pub session_id: Uuid,
    pub content_type: String,
    pub text: String,
    pub similarity: f32,
    pub importance: f32,
    pub combined_score: f32,
    pub timestamp: DateTime<Utc>,
}

/// Structured / keyword query against the session's context updates.
/// The `query_type` enum mirrors the legacy MCP `query_conversation_context`
/// `query_type` parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryContextRequest {
    pub session_id: Uuid,
    pub query_type: String,
    pub parameters: HashMap<String, String>,
}

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
    pub query: String,
    pub scope: SearchScope,
    pub max_tokens: Option<usize>,
    pub include_impact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembleContextResponse {
    pub formatted_text: String,
    pub total_tokens: usize,
    pub entity_context: Vec<EntityContextItem>,
    pub items: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityContextItem {
    pub entity_name: String,
    pub entity_type: String,
    pub importance: f32,
    pub mentions: u32,
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionAction {
    Create {
        name: Option<String>,
        description: Option<String>,
    },
    List,
    Load {
        session_id: Uuid,
    },
    Search {
        query: String,
    },
    UpdateMetadata {
        session_id: Uuid,
        name: Option<String>,
        description: Option<String>,
    },
    Delete {
        session_id: Uuid,
    },
    CreateCheckpoint {
        session_id: Uuid,
    },
    LoadCheckpoint {
        checkpoint_id: String,
        session_id: Uuid,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageSessionRequest {
    pub action: SessionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageSessionResponse {
    pub data: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceAction {
    Create {
        name: String,
        description: String,
        session_ids: Vec<Uuid>,
    },
    List,
    Get {
        workspace_id: Uuid,
    },
    Delete {
        workspace_id: Uuid,
    },
    AddSession {
        workspace_id: Uuid,
        session_id: Uuid,
        role: Option<String>,
    },
    RemoveSession {
        workspace_id: Uuid,
        session_id: Uuid,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageWorkspaceRequest {
    pub action: WorkspaceAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageWorkspaceResponse {
    pub data: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Entity maintenance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntityAction {
    /// Delete an entity and cascade through the entity graph.
    Delete {
        session_id: Uuid,
        entity_name: String,
    },
    /// Delete a single context-update entry (removes from caches +
    /// persistent storage).
    DeleteUpdate {
        session_id: Uuid,
        entry_id: Uuid,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageEntityRequest {
    pub action: EntityAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageEntityResponse {
    pub success: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Analytics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSummaryRequest {
    pub session_id: Uuid,
    pub compact: bool,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdminAction {
    /// Run the embedding pipeline over every update in a session that
    /// hasn't been vectorised yet.
    VectorizeSession { session_id: Uuid },
    /// Snapshot a session to a serialisable checkpoint.
    CreateCheckpoint { session_id: Uuid },
    /// Fetch vectorisation stats (total / vectorised / pending counts).
    VectorizationStats,
    /// Health probe; mirrors [`PostCortexService::health`] but routed
    /// through the admin surface so MCP/REST clients can call it
    /// uniformly with the rest of the admin tools.
    Health,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminRequest {
    pub action: AdminAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminResponse {
    pub success: bool,
    pub message: String,
    pub data: serde_json::Value,
}
