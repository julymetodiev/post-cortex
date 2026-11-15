// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! The runtime state of a memory instance ([`MemoryRuntime`]): the session
//! cache, the workspace registry, the persister, the derived-work counter,
//! the vectorizer and the models, with the few helpers several capability
//! services (`crate::capabilities`) share. The operations themselves are
//! the services' methods.

#[cfg(test)]
pub mod backends_tests;
/// Circuit-breaker pattern for graceful degradation under sustained failures.
/// Operation types and system configuration
mod config;
/// Vectors of content that was removed
mod content_vectors;
#[cfg(test)]
mod delete_tests;
/// Derived work (embeddings, graph merges) in flight
pub mod derived_work;
#[cfg(test)]
mod derived_work_tests;
mod document_vectors;
#[cfg(test)]
mod durability_tests;
mod embeddings;

/// Record change notification ([`MemoryRuntime::subscribe_records`])
pub mod change_log;
mod error;
#[cfg(test)]
mod eviction_tests;
#[cfg(test)]
mod freshness_tests;
#[cfg(test)]
mod ingest_bench_tests;
mod keyed_locks;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod message_vector_tests;
mod message_vectors;
/// System-level and session-level metrics snapshots
mod metrics;
mod neighbours;
/// Coalescing, per-session ordered session persistence
mod persister;
#[cfg(test)]
mod record_event_tests;
mod record_events;
#[cfg(test)]
mod remote_tests;
mod rerankers;
/// Cached session handles with pinned, least-recently-used eviction
mod session_cache;
#[cfg(test)]
mod session_cache_tests;
/// The compare-and-swap loop of session changes
mod session_cas;
/// The session cache and per-session derived state
mod session_manager;
mod session_ops;
#[cfg(test)]
mod session_required_tests;
/// Top-level conversation memory system orchestrator
mod system;
#[cfg(test)]
mod transfer_tests;
mod update_vectors;
#[cfg(test)]
mod vectorize_batch_tests;
mod vectorized;
/// Merged workspace graphs (per instance, bounded)
pub mod workspace_graphs;
/// The workspace registry (runtime state)
mod workspace_registry;
#[cfg(test)]
mod workspace_registry_tests;
#[cfg(test)]
pub mod write_path_tests;
#[cfg(test)]
mod write_record_tests;

pub use change_log::{
    CardChange, Change, ChangeLog, DocumentChange, MessagesChange, MessagesChangeKind,
    WorkspaceChange, WorkspaceChangeKind,
};
pub use config::{DEFAULT_MAX_IMPORT_BYTES, SystemConfig};
pub use embeddings::VectorizeTarget;
pub use error::RuntimeError;
pub use keyed_locks::{KeyedGuard, KeyedLocks};
pub use neighbours::complete_results;
pub use persister::PersistError;
pub use record_events::{RecordEvent, RecordEventKind};
pub use session_cache::SessionHandle;
pub use system::MemoryRuntime;
pub use vectorized::VectorizedIds;
pub use workspace_graphs::WorkspaceGraphCache;
pub use workspace_registry::RegistryError;
pub use workspace_registry::{RestoredWorkspace, WorkspaceManager};
