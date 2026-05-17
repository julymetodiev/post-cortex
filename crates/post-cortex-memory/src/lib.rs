// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Conversation memory orchestrator for post-cortex.
//!
//! This crate is the **top of the dependency tree** — it pulls in
//! [`post_cortex_core`] (domain types), [`post_cortex_storage`]
//! (RocksDB + SurrealDB backends), [`post_cortex_embeddings`] (BERT +
//! HNSW), and [`post_cortex_proto`] (gRPC wire types). It owns
//! [`ConversationMemorySystem`], the lock-free hot/warm/cold memory
//! hierarchy that ties everything together, plus the content
//! vectorizer pipeline, semantic query engine, query cache, graph-aware
//! context assembly, and the relevance scoring helpers.
//!
//! Phase 4 of the migration adds a canonical `PostCortexService` trait
//! impl here so MCP/gRPC transports can delegate to a single internal
//! handler per operation (TODO.md:106-117). Phase 5 adds the non-
//! blocking pipeline work queues (TODO.md:136-145).

#![forbid(unsafe_code)]
// SystemError + memory::Error carry rich variants for transport layers.
// Boxing them all to satisfy result_large_err would force every caller
// to deref before pattern matching.
#![allow(clippy::result_large_err)]
// Storage actor request/response types are complex tuples by design.
#![allow(clippy::type_complexity)]
// `ptr_arg` flags a &mut Vec param that could be &mut [_], but the
// caller relies on push() — slicing would break the contract.
#![allow(clippy::ptr_arg)]

pub mod content_vectorizer;
/// Graph-aware context assembly that combines semantic search results with entity-graph traversals.
pub mod context_assembly;
pub mod error;
pub mod memory_system;
/// Runtime performance monitoring: operation timers, cache trackers, and health snapshots.
pub mod performance;
pub mod pipeline;
pub mod query_cache;
pub mod scoring;
pub mod semantic_query_engine;
pub mod services;

/// Canonical error type and result alias for this crate.
pub use error::{Error, Result};
/// Top-level memory orchestrator and its configuration.
pub use memory_system::{ConversationMemorySystem, SystemConfig};
/// Non-blocking write pipeline, configuration, and error types.
pub use pipeline::{Pipeline, PipelineConfig, PipelineError};
/// gRPC / MCP service implementation backed by [`ConversationMemorySystem`].
pub use services::MemoryServiceImpl;
