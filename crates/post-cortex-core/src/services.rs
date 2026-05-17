// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Canonical service trait for post-cortex.
//!
//! `PostCortexService` is the **single internal entrypoint** that every
//! transport layer (gRPC, MCP, REST, future SDKs) delegates to. Per
//! TODO.md:106-117 we never want two transports re-implementing the same
//! operation with subtly different validation; they each translate their
//! wire payload, then call the same method on this trait. The canonical
//! impl lives in [`post-cortex-memory`](https://docs.rs/post-cortex-memory)
//! as `MemoryServiceImpl`.
//!
//! ## Scope
//!
//! Phase 4 introduces the trait skeleton with the **read/write/search/manage**
//! operations every transport exposes. Phases 6 + 7 (MCP and daemon
//! extraction) migrate the existing handlers to delegate here; until then
//! both transports still call into `ConversationMemorySystem` directly.
//!
//! Trait methods intentionally take small, immediately-usable request
//! types defined in `types` rather than huge proto structs — keeping
//! the surface readable from non-gRPC consumers. gRPC handlers do the
//! proto-to-domain translation in a single `From`/`Into` module at the
//! transport boundary (Phase 7).
//!
//! ## Object safety
//!
//! The trait is intentionally object-safe (no generic methods, no `Self`
//! return types) so consumers — `post-cortex-mcp` in particular — can
//! hold an `Arc<dyn PostCortexService>` and let downstream Rust projects
//! plug in their own implementation without touching the MCP layer.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::error::SystemError;

pub mod types;

pub use types::*;

/// Canonical post-cortex service.
///
/// Every transport (gRPC, MCP, REST, future SDKs) delegates to this trait
/// so there is exactly one implementation per operation across the
/// codebase. See the module-level docs for the design rationale.
///
/// Default implementations are kept minimal — the production impl is
/// `post_cortex_memory::services::MemoryServiceImpl`. Downstream Rust
/// projects can implement this trait against their own storage +
/// embeddings stack and reuse the rest of the post-cortex ecosystem
/// (MCP tools, gRPC client, summary view).
#[async_trait]
pub trait PostCortexService: Send + Sync + 'static {
    // ------------------------------------------------------------------
    // Liveness
    // ------------------------------------------------------------------

    /// Returns a snapshot of system health. Cheap — the only operation
    /// that does **not** flow through the storage actor and is safe to
    /// call from a heartbeat probe.
    async fn health(&self) -> Result<HealthReport, SystemError>;

    // ------------------------------------------------------------------
    // Write path
    // ------------------------------------------------------------------

    /// Persist a single context update for a session and enqueue derived
    /// work (embedding, HNSW upsert, graph update, summary refresh)
    /// onto background pipelines. Returns once the entry is durably
    /// persisted; embeddings / vector index land asynchronously per
    /// TODO.md:136-145.
    async fn update_context(
        &self,
        req: UpdateContextRequest,
    ) -> Result<UpdateContextResponse, SystemError>;

    /// Batch variant of [`Self::update_context`]. Backends use storage
    /// write batches when available (RocksDB transaction).
    async fn bulk_update_context(
        &self,
        req: BulkUpdateContextRequest,
    ) -> Result<BulkUpdateContextResponse, SystemError>;

    // ------------------------------------------------------------------
    // Read path
    // ------------------------------------------------------------------

    /// Run a semantic similarity search against persisted content. Scope
    /// determines whether the query runs against a single session, a
    /// workspace, or the global index.
    async fn semantic_search(
        &self,
        req: SemanticSearchRequest,
    ) -> Result<SemanticSearchResponse, SystemError>;

    /// Run a structured/keyword query against the session's context
    /// updates. Faster than semantic search; no embedding required.
    async fn query_context(
        &self,
        req: QueryContextRequest,
    ) -> Result<QueryContextResponse, SystemError>;

    /// Graph-aware retrieval: semantic search + entity neighbourhood
    /// traversal + impact analysis, all merged into one assembled
    /// context payload.
    async fn assemble_context(
        &self,
        req: AssembleContextRequest,
    ) -> Result<AssembleContextResponse, SystemError>;

    // ------------------------------------------------------------------
    // Session management
    // ------------------------------------------------------------------

    /// Manage session lifecycle (create / list / load / search / update /
    /// delete). The variant carried in the request determines which
    /// operation runs — letting callers (especially MCP) treat sessions
    /// as a single tool surface.
    async fn manage_session(
        &self,
        req: ManageSessionRequest,
    ) -> Result<ManageSessionResponse, SystemError>;

    // ------------------------------------------------------------------
    // Workspace management
    // ------------------------------------------------------------------

    /// Manage workspace lifecycle (create / list / get / delete /
    /// add-session / remove-session). Same shape as
    /// [`Self::manage_session`].
    async fn manage_workspace(
        &self,
        req: ManageWorkspaceRequest,
    ) -> Result<ManageWorkspaceResponse, SystemError>;

    // ------------------------------------------------------------------
    // Entity maintenance
    // ------------------------------------------------------------------

    /// Manage entity lifecycle (delete / delete-update). Cascades through
    /// the entity graph; storage-layer side effects (vector index purge,
    /// summary invalidation) run on background pipelines.
    async fn manage_entity(
        &self,
        req: ManageEntityRequest,
    ) -> Result<ManageEntityResponse, SystemError>;

    // ------------------------------------------------------------------
    // Analytics & summaries
    // ------------------------------------------------------------------

    /// Compose the structured summary view for a session — a
    /// hierarchical projection over context updates, decisions,
    /// problems, and entity importance.
    async fn get_structured_summary(
        &self,
        req: StructuredSummaryRequest,
    ) -> Result<StructuredSummaryResponse, SystemError>;

    // ------------------------------------------------------------------
    // Admin
    // ------------------------------------------------------------------

    /// Run an admin operation (vectorize session, create checkpoint,
    /// fetch vectorization stats, etc.). Variant-dispatched like
    /// session/workspace/entity for symmetry.
    async fn admin(&self, req: AdminRequest) -> Result<AdminResponse, SystemError>;
}

/// Convenience alias — most consumers carry the trait behind an `Arc`.
pub type DynPostCortexService = Arc<dyn PostCortexService>;
