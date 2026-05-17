// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Core domain library for post-cortex — types, error, and lightweight
//! domain modules.
//!
//! This crate is the **dependency root** of the workspace. It contains
//! only the types and helpers needed to describe a post-cortex
//! conversation: session, graph, workspace, summary, and the
//! cross-cutting primitives under [`core`]. Heavy infrastructure lives
//! in dedicated sibling crates:
//!
//! | Concern | Crate |
//! |--------|-------|
//! | gRPC wire types | [`post_cortex_proto`] |
//! | Storage backends (RocksDB, SurrealDB) | `post-cortex-storage` |
//! | Embeddings + HNSW | `post-cortex-embeddings` |
//! | Conversation memory orchestrator | `post-cortex-memory` |
//! | MCP tool library | `post-cortex-mcp` |
//! | rmcp + axum + tonic daemon | `post-cortex-daemon` |
//!
//! Library users embedding post-cortex into their own Rust applications
//! depend on `post-cortex-core` if they want the type system only, or
//! on `post-cortex-memory` (which transitively pulls core + storage +
//! embeddings) if they want a ready-to-use memory system.
//!
//! See `/Users/julius/.claude/plans/stateful-hugging-hopper.md` for the
//! migration plan that produced this layout.

// candle's `from_mmaped_safetensors` requires `unsafe` at a single call
// site in post-cortex-embeddings; this crate is otherwise unsafe-free.
#![deny(unsafe_code)]
// Cosmetic clippy suggestions silenced workspace-wide — see other
// crates' lib.rs for the same rationale.
#![allow(clippy::result_large_err)]
#![allow(clippy::type_complexity)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::manual_div_ceil)]
// `sort_by` with .cmp() is sometimes clearer than the suggested
// `sort_by_key(|x| x.field)` when the key involves multiple fields.
#![allow(clippy::unnecessary_sort_by)]
// Doc lists with indentation drift in legacy code; cleanup is Phase 12
// follow-up.
#![allow(clippy::doc_lazy_continuation)]

/// Cross-cutting primitives shared by every post-cortex crate.
///
/// Hosts the [`cache`][core::cache] LRU helpers, the
/// [`context_update`][core::context_update] data model, the
/// [`error`][core::error] hierarchy ([`SystemError`] + [`Result`]),
/// the [`structured_context`][core::structured_context] projection
/// types, and the [`timeout_utils`][core::timeout_utils] retry helpers.
pub mod core;

/// Entity graph + GraphRAG types — `petgraph`-backed knowledge graph
/// used to enrich semantic search with relationship traversal.
pub mod graph;

/// Canonical [`services::PostCortexService`] trait. Every transport
/// (gRPC, MCP, REST) delegates to the same single implementation in
/// `post-cortex-memory`.
pub mod services;

/// `ActiveSession` and session-component types — the hot-context cache
/// + change-history layer that backs the lock-free session manager.
pub mod session;

/// Read-only summary projection types (decisions, entities,
/// timeline) consumed by MCP `get_structured_summary` and the gRPC
/// equivalent.
pub mod summary;

/// Workspace types — `WorkspaceManager`, `SessionRole`, the entities
/// that group multiple sessions for cross-session search.
pub mod workspace;

pub use crate::core::error::{Result, SystemError};

/// Re-export of the gRPC wire-types crate so consumers can reach freshness /
/// source-reference types as `post_cortex_core::proto::FreshnessEntry` —
/// matches the convenience export of the legacy single-crate layout.
pub use post_cortex_proto as proto;
