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

pub mod content_vectorizer;
pub mod context_assembly;
pub mod memory_system;
pub mod performance;
pub mod query_cache;
pub mod scoring;
pub mod semantic_query_engine;

pub use memory_system::{ConversationMemorySystem, SystemConfig};
