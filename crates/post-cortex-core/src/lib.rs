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
//! | gRPC wire types | `post-cortex-proto` |
//! | Port traits and the types they speak | `post-cortex-ports` |
//! | Storage backends (RocksDB, SurrealDB) | `post-cortex-storage` |
//! | Embeddings + HNSW | `post-cortex-embeddings` |
//! | Conversation memory orchestrator | `post-cortex-memory` |
//! | Operation table, MCP server | `post-cortex-transport` |
//! | rmcp + axum + tonic daemon | `post-cortex-daemon` |
//!
//! Library users embedding post-cortex into their own Rust applications
//! depend on `post-cortex-core` if they want the type system only, or
//! on `post-cortex-memory` (which transitively pulls core + storage +
//! embeddings) if they want a ready-to-use memory system.

// candle's `from_mmaped_safetensors` requires `unsafe` at a single call
// site in post-cortex-embeddings; this crate is otherwise unsafe-free.
#![deny(unsafe_code)]

pub mod archive;
pub mod collections;

/// Cross-cutting primitives shared by every post-cortex crate.
///
/// Hosts the [`context_update`][core::context_update] data model, the
/// [`content`][core::content] kinds, the
/// [`structured_context`][core::structured_context] derivation and the
/// [`wire`][core::wire] enum codec.
pub mod core;

/// Entity graph types — the knowledge graph of a session
/// used to enrich semantic search with relationship traversal.
pub mod graph;

/// Secret and personal-data redaction applied on the canonical write path.
pub mod redact;

/// Request and response DTOs of the memory capability services
/// (`post_cortex_memory::capabilities`).
pub mod services;

/// `ActiveSession` and session-component types — the hot-context cache
/// + change-history layer that backs the lock-free session manager.
pub mod session;

/// Read-only summary projection types (decisions, entities,
/// timeline) consumed by MCP `get_structured_summary` and the gRPC
/// equivalent.
pub mod summary;
/// The tokenizer every lexical (BM25) index shares.
pub mod text;

/// Slugs of workspaces (and the cloud's organisations): made once from the
/// name, kept after a rename.
pub mod slug;

/// Reading times callers write.
pub mod time;

/// Workspace types — `Workspace`, `SessionRole`, the entities
/// that group multiple sessions for cross-session search.
pub mod workspace;
