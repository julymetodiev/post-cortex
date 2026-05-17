// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Legacy `post-cortex` single-crate facade (transitional).
//!
//! Phases 3-7 of the workspace refactor moved every library module out
//! of this crate. What remains is a thin re-export layer that keeps the
//! historical import paths (`post_cortex::ConversationMemorySystem`,
//! `post_cortex::storage::*`, `post_cortex::tools::mcp::*`, etc.)
//! resolving so the integration tests under `tests/` keep compiling
//! against the published surface.
//!
//! Phase 8 retires this crate entirely: the new `post-cortex` facade
//! crate at `crates/post-cortex/` takes over the published name and the
//! workspace root drops its [`[package]`] section.

pub use post_cortex_core::core;
pub use post_cortex_core::graph;
pub use post_cortex_core::session;
pub use post_cortex_core::summary;
pub use post_cortex_core::workspace;
pub use post_cortex_storage as storage;

/// Phase 6 — `tools::mcp` is published as `post-cortex-mcp`. The shim
/// here preserves the historical `post_cortex::tools::mcp::X` path for
/// integration tests + downstream callers until Phase 8.
pub mod tools {
    pub use post_cortex_mcp as mcp;
}

/// Phase 7 — the daemon is published as `post-cortex-daemon`. Legacy
/// `post_cortex::daemon::X` callers keep working through this
/// re-export.
pub use post_cortex_daemon::daemon;

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
