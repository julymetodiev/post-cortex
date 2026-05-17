// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Pure domain primitives shared by every post-cortex crate.
//!
//! After Phase 3 the orchestrator (`memory_system`, `content_vectorizer`,
//! `semantic_query_engine`, `query_cache`, `context_assembly`,
//! `scoring`, `performance`) lives in `post-cortex-memory`; the
//! embedding engines + HNSW index live in `post-cortex-embeddings`; the
//! storage backends live in `post-cortex-storage`. What remains here is
//! transport-/IO-free domain logic.

/// Generic LRU cache primitives used across the workspace.
pub mod cache;

/// Context-update data model — `ContextUpdate`, `EntityData`,
/// `EntityRelationship`, `UpdateType`, and supporting types.
pub mod context_update;

/// Typed system error hierarchy and `Result<T>` alias.
pub mod error;

/// Structured projection types over a session's update history.
pub mod structured_context;

/// Retry + timeout helpers (`with_storage_timeout`, `with_mcp_timeout`).
pub mod timeout_utils;
