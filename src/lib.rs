// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Legacy single-crate root for post-cortex (transitional facade).
//!
//! Phases 3a-3d of the workspace refactor moved every library module out
//! of this crate:
//!
//! | Old path | New crate |
//! |----------|-----------|
//! | `crate::core::{cache,context_update,error,structured_context,timeout_utils}` | `post-cortex-core` |
//! | `crate::core::{memory_system,content_vectorizer,context_assembly,scoring,semantic_query_engine,query_cache,performance}` | `post-cortex-memory` |
//! | `crate::core::{embeddings,vector_db}` | `post-cortex-embeddings` |
//! | `crate::{session,graph,workspace,summary}` | `post-cortex-core` |
//! | `crate::storage` | `post-cortex-storage` |
//! | `proto::pb` (daemon's old generated module) | `post-cortex-proto::pb` |
//!
//! This file re-exports the moved modules under their original paths so
//! `src/daemon/` and `src/tools/` — which still live in this crate
//! until Phases 6 and 7 — keep compiling without touching every `use`
//! statement. The shim is removed in Phase 8 when the legacy package
//! becomes the `post-cortex` facade meta-crate.

/// `crate::core::*` shim. Combines the pure-domain modules from
/// post-cortex-core with the orchestrator pieces from post-cortex-memory
/// and the embeddings stack from post-cortex-embeddings so historical
/// import paths under `crate::core::*` keep resolving.
pub mod core {
    pub use post_cortex_core::core::{
        cache, context_update, error, structured_context, timeout_utils,
    };
    pub use post_cortex_memory::{
        content_vectorizer, context_assembly, memory_system, performance, query_cache, scoring,
        semantic_query_engine,
    };
    // post-cortex-embeddings is the single crate hosting both the
    // EmbeddingBackend stack and the HNSW vector_db. Aliasing it twice
    // preserves the legacy `crate::core::embeddings::*` and
    // `crate::core::vector_db::*` paths.
    pub use post_cortex_embeddings as embeddings;
    pub use post_cortex_embeddings::vector_db;
}

pub use post_cortex_core::graph;
pub use post_cortex_core::session;
pub use post_cortex_core::summary;
pub use post_cortex_core::workspace;
pub use post_cortex_storage as storage;

// Modules still hosted here. Phase 6 moves tools/mcp into post-cortex-mcp;
// Phase 7 moves daemon + pcx bin into post-cortex-daemon.
pub mod daemon;
pub mod tools;

// Headline re-exports preserved verbatim from the pre-Phase-3 surface.
pub use post_cortex_core::core::error::{Result, SystemError};
pub use post_cortex_core::summary::{StructuredSummaryView, SummaryGenerator};
pub use post_cortex_memory::{ConversationMemorySystem, SystemConfig};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_system_initialization() {
        let test_dir = format!(
            "./test_data_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::fs::create_dir_all(&test_dir).unwrap();

        let config = SystemConfig {
            data_directory: test_dir.clone(),
            ..Default::default()
        };

        let system = ConversationMemorySystem::new(config).await;
        assert!(system.is_ok());

        std::fs::remove_dir_all(&test_dir).unwrap();
    }
}
