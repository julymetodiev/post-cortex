// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `post-cortex` — facade meta-crate.
//!
//! One dependency for the whole post-cortex stack. Re-exports the
//! workspace member crates so callers can write
//! `use post_cortex::{ConversationMemorySystem, SystemConfig};` or
//! reach the canonical service trait via `post_cortex::PostCortexService`.
//!
//! ## Picking the right crate
//!
//! Direct dependence on a single member crate is usually preferable —
//! it scopes the build to exactly what you need:
//!
//! | Goal | Crate |
//! |------|-------|
//! | Domain types, traits, error | [`post_cortex_core`] |
//! | gRPC client (no server) | [`post_cortex_proto`] |
//! | Custom storage backend | [`post_cortex_storage`] |
//! | Embedding engine + HNSW | [`post_cortex_embeddings`] |
//! | Full memory orchestrator | [`post_cortex_memory`] |
//! | MCP tool functions | [`post_cortex_mcp`] |
//! | Running daemon + `pcx` CLI | [`post_cortex_daemon`] |
//!
//! This facade is for **end-to-end consumers** (e.g. integration
//! tests, sample apps) that want the entire surface without picking.

#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]

// Core domain — types, error, graph/session/workspace/summary helpers,
// the `core::*` cross-cutting primitives, and the canonical
// `PostCortexService` trait.
pub use post_cortex_core::*;

// Storage backends (RocksDB + optional SurrealDB). Re-exported under
// `post_cortex::storage::*`.
pub use post_cortex_storage as storage;

// Embedding engines + HNSW vector database.
pub use post_cortex_embeddings as embeddings;

// Memory orchestrator: `ConversationMemorySystem`, `SystemConfig`,
// `MemoryServiceImpl`, the non-blocking write `Pipeline`.
pub use post_cortex_memory::{
    ConversationMemorySystem, MemoryServiceImpl, Pipeline, PipelineConfig, PipelineError,
    SystemConfig,
};

/// MCP tool functions. Re-exported under `post_cortex::mcp` so
/// downstream callers writing custom MCP servers can embed them.
pub mod mcp {
    pub use post_cortex_mcp::*;
}

/// `tools::mcp::*` historical path — preserved as a convenience for
/// existing code; new callers should use [`mcp`] directly.
pub mod tools {
    pub use post_cortex_mcp as mcp;
}

/// Daemon runtime (axum + tonic + rmcp + SSE). Pulled in transitively
/// by depending on this crate; reach individual pieces under
/// `post_cortex::daemon::*`. The `pcx` CLI binary is shipped by
/// `post-cortex-daemon` directly via its `[[bin]]` entry — depend on
/// that crate (not this facade) if you only want to run the daemon.
pub use post_cortex_daemon::daemon;

/// Re-export the proto wire types under `post_cortex::proto`.
pub use post_cortex_core::proto;
