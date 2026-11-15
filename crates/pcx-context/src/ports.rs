// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the context engine needs of memory besides the models: what it
//! recalls, and where a summary is kept.

use async_trait::async_trait;
use pcx_module_api::ModuleError;

/// Memory, for the context engine.
#[async_trait]
pub trait Memory: Send + Sync {
    /// What memory recalls in `session` for `query`: the number of items
    /// and their LLM-ready text.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn recall(&self, session: &str, query: &str) -> Result<(usize, String), ModuleError>;

    /// Keep `summary` as a record of `session`.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn keep_summary(&self, session: &str, summary: &str) -> Result<(), ModuleError>;
}
