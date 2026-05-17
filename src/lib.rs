// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Legacy single-crate root for post-cortex.
//!
//! As of Phase 3 of the workspace refactor, the heavy domain modules
//! (`core`, `storage`, `session`, `graph`, `summary`, `workspace`) have
//! moved to [`post_cortex_core`]. They are re-exported here unchanged so
//! the still-in-tree `daemon` and `tools` modules — plus integration
//! tests under `tests/` — can keep using `crate::core::X`,
//! `crate::storage::X`, etc. paths until Phases 6–8 finish the split.
//!
//! See `/Users/julius/.claude/plans/stateful-hugging-hopper.md`.

// Re-export the post-cortex-core modules so legacy `crate::core::X` paths
// resolve via the workspace member instead of in-tree code.
pub use post_cortex_core::core;
pub use post_cortex_core::graph;
pub use post_cortex_core::session;
pub use post_cortex_core::storage;
pub use post_cortex_core::summary;
pub use post_cortex_core::workspace;

// Modules still living in this legacy crate. Phase 6 moves `tools/mcp`
// into post-cortex-mcp; Phase 7 moves `daemon` into post-cortex-daemon.
pub mod daemon;
pub mod tools;

// Headline re-exports preserved verbatim from the pre-Phase-3 surface so
// `post_cortex::ConversationMemorySystem` etc. keep resolving for
// integration tests and the `pcx` CLI.
pub use post_cortex_core::core::error::{Result, SystemError};
pub use post_cortex_core::core::memory_system::{ConversationMemorySystem, SystemConfig};
pub use post_cortex_core::summary::{StructuredSummaryView, SummaryGenerator};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_system_initialization() {
        // Create a unique directory for this test
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

        // Cleanup
        std::fs::remove_dir_all(&test_dir).unwrap();
    }
}
