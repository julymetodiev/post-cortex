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

pub mod core;
pub mod graph;
pub mod services;
pub mod session;
pub mod summary;
pub mod workspace;

pub use crate::core::error::{Result, SystemError};

/// Re-export of the gRPC wire-types crate so consumers can reach freshness /
/// source-reference types as `post_cortex_core::proto::FreshnessEntry` —
/// matches the convenience export of the legacy single-crate layout.
pub use post_cortex_proto as proto;
