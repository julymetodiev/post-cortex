// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `post-cortex` — facade meta-crate.
//!
//! One dependency for the whole post-cortex stack. Re-exports the
//! workspace member crates so callers can write
//! `use post_cortex::{Memory, SystemConfig};` and reach every capability
//! service through it (`memory.records`, `memory.search`, ...).
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
//! | Full memory orchestrator, its tools and module | [`post_cortex_memory`] |
//! | MCP server over the module host | [`post_cortex_transport`] |
//! | Running daemon + `pcx` CLI | [`post_cortex_daemon`] |
//!
//! This facade is for **end-to-end consumers** (e.g. integration
//! tests, sample apps) that want the entire surface without picking.

#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]

// Core domain — types, graph/session/workspace/summary helpers, the
// `core::*` cross-cutting primitives, and the capability request DTOs.
pub use post_cortex_core::*;

// Storage backends (RocksDB + optional SurrealDB). Re-exported under
// `post_cortex::storage::*`.
pub use post_cortex_storage as storage;

/// The port traits and the types they speak.
pub use post_cortex_ports as ports;

// Embedding engines + HNSW vector database.
pub use post_cortex_embeddings as embeddings;

// Memory: the capability services (`Memory` builds them all), their
// configuration and the embedding pipeline's; the memory tools (operation
// table, response shapes, `select`, typed failures) and `MemoryModule`.
pub use post_cortex_memory::{
    Memory, MemoryModule, MemoryPorts, PipelineConfig, SystemConfig, capabilities, tools,
};

/// Transports: the MCP server over the module host
/// (`post_cortex::transport::mcp`) and the gRPC failure mapping.
pub use post_cortex_transport as transport;

/// Daemon runtime (axum + tonic + rmcp). Pulled in transitively
/// by depending on this crate; reach individual pieces under
/// `post_cortex::daemon::*`. The `pcx` CLI binary is shipped by
/// `post-cortex-daemon` directly via its `[[bin]]` entry — depend on
/// that crate (not this facade) if you only want to run the daemon.
pub use post_cortex_daemon::daemon;

/// The composition root: open the adapters a configuration names and build
/// [`Memory`] over them (`compose::open_memory`).
pub use post_cortex_daemon::compose;

/// Re-export the proto wire types under `post_cortex::proto`.
pub use post_cortex_proto as proto;
