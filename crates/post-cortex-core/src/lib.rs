// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Core domain library for post-cortex.
//!
//! Owns the lock-free conversation memory system, semantic search engine,
//! entity graph, storage backends, session state machine, summary
//! generator, workspace manager, and (Phase 4) the canonical
//! `PostCortexService` trait that transports — gRPC, MCP — delegate to.
//!
//! # Public surface
//!
//! ```text
//! pub mod core      — memory_system, cache, embeddings, vector_db, scoring, ...
//! pub mod storage   — Storage trait + RocksDB / SurrealDB backends
//! pub mod session   — ActiveSession + components
//! pub mod graph     — EntityGraph + GraphRAG
//! pub mod summary   — StructuredSummaryView + SummaryGenerator
//! pub mod workspace — WorkspaceManager
//! pub use post_cortex_proto as proto;
//! ```
//!
//! The headline re-exports (`ConversationMemorySystem`, `SystemConfig`,
//! `SystemError`, `Result`) live at the crate root so most consumers
//! never need to traverse the module tree.

// Workspace-wide policy is `forbid(unsafe_code)`, but candle-core's
// `VarBuilder::from_mmaped_safetensors` is an `unsafe fn` (memory mapping
// a model file from disk), so we relax to `deny` and gate the single
// legitimate call site with `#[allow(unsafe_code)]` and a SAFETY comment.
// No other module needs unsafe; clippy will flag any new usage.
#![deny(unsafe_code)]
// The strict lint suite (missing_docs, clippy::pedantic, clippy::nursery,
// unwrap_used, expect_used, panic) lands in Phase 9 (error typing) and
// Phase 12 (docs pass). Until then the implicit clippy defaults apply so
// the workspace migration stays bisectable.

pub mod core;
pub mod graph;
pub mod session;
pub mod storage;
pub mod summary;
pub mod workspace;

pub use crate::core::error::{Result, SystemError};
pub use crate::core::memory_system::{ConversationMemorySystem, SystemConfig};
pub use crate::summary::{StructuredSummaryView, SummaryGenerator};

/// Re-export of [`post_cortex_proto`] so downstream consumers can reach
/// wire types as `post_cortex_core::proto::FreshnessEntry` without adding
/// an explicit dependency on the proto crate when they only use it
/// transitively.
pub use post_cortex_proto as proto;
