// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Conversation memory orchestrator for post-cortex.
//!
//! It depends on [`post_cortex_core`] (domain types) and
//! [`post_cortex_ports`] (the ports) only: the composition root opens the
//! adapters and hands them over as [`MemoryPorts`]. Its API is the
//! capability services of [`capabilities`] (built together by
//! [`Memory`]); they run on one runtime (session cache, workspace
//! registry, persister, the bounded embedding pipeline), with the content
//! vectorizer, query cache, graph-aware context assembly and the relevance
//! scoring helpers.

#![forbid(unsafe_code)]

pub mod archive;
pub mod capabilities;
pub mod content_vectorizer;
/// Graph-aware context assembly that combines semantic search results with entity-graph traversals.
pub mod context_assembly;
pub mod graph_views;
mod memory_system;
mod pipeline;
pub mod query_cache;
pub mod reach;
mod scoring;
pub mod tools;

/// Every capability service of one memory instance, and the ports it runs
/// on.
pub use capabilities::{Memory, MemoryPorts, OpenError};
/// Capability configuration.
pub use memory_system::{
    DEFAULT_MAX_IMPORT_BYTES, PersistError, RecordEvent, RecordEventKind, RegistryError,
    RuntimeError, SystemConfig,
};
pub use pipeline::PipelineConfig;
/// The bounded embedding pipeline's configuration.
pub use tools::MemoryModule;
