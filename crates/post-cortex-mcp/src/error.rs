// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Typed error hierarchy for `post-cortex-mcp`.
//!
//! Wraps lower-layer errors and adds MCP-specific variants
//! (validation, JSON-RPC formatting). The [`Error::mcp_code`] method
//! maps each variant to a JSON-RPC error code transport adapters can
//! use directly.

use thiserror::Error;

/// Errors produced by the MCP tool layer.
#[derive(Debug, Error)]
pub enum Error {
    /// Caller's MCP tool params failed validation.
    #[error("validation error: {0}")]
    Validation(String),

    /// Underlying memory / orchestrator error.
    #[error(transparent)]
    Memory(#[from] post_cortex_memory::error::Error),

    /// Underlying storage error (exposed when the MCP tool calls into
    /// storage directly, e.g. session list).
    #[error(transparent)]
    Storage(#[from] post_cortex_storage::error::Error),

    /// JSON (de)serialization failed at the MCP boundary.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Catch-all for migrating call sites that still use `anyhow`.
    #[error(transparent)]
    External(#[from] anyhow::Error),
}

/// Crate-level result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// A stable discriminant for metrics / structured logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation",
            Self::Memory(_) => "memory",
            Self::Storage(_) => "storage",
            Self::Json(_) => "json",
            Self::External(_) => "external",
        }
    }

    /// Map to a JSON-RPC error code per the MCP spec.
    ///
    /// - `-32602` Invalid params (validation failures).
    /// - `-32603` Internal error (everything else).
    #[must_use]
    pub fn mcp_code(&self) -> i32 {
        match self {
            Self::Validation(_) => -32602,
            _ => -32603,
        }
    }
}
