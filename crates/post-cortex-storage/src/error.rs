// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Typed error hierarchy for `post-cortex-storage`.
//!
//! See the crate-level docs on `post-cortex-core::error` for the
//! workspace-wide error policy. Storage errors carry source chains via
//! `#[source]` so callers can pattern-match on the underlying RocksDB /
//! SurrealDB cause.

use thiserror::Error;

/// Errors produced by storage backends.
#[derive(Debug, Error)]
pub enum Error {
    /// RocksDB-layer error (transaction, write batch, iterator, column
    /// family).
    #[error("rocksdb error: {0}")]
    RocksDb(#[from] rocksdb::Error),

    /// SurrealDB error (kv-mem / kv-tikv / protocol-ws).
    #[cfg(feature = "surrealdb-storage")]
    #[error("surrealdb error: {0}")]
    SurrealDb(String),

    /// (De)serialization failure on a persisted record.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Logical "not found" — referenced session / workspace / entity
    /// missing from the backend.
    #[error("not found: {kind} {id}")]
    NotFound {
        /// Kind of entity that was missing (e.g. `"session"`, `"workspace"`).
        kind: &'static str,
        /// Identifier of the missing entity.
        id: String,
    },

    /// Export / import file format error (corrupt header, unsupported
    /// version, compression failure).
    #[error("export/import format error: {0}")]
    Format(String),

    /// I/O error from the underlying filesystem.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Catch-all for migrating call sites that still use `anyhow`.
    #[error(transparent)]
    External(#[from] anyhow::Error),
}

/// Crate-level result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

// bincode 2 conversions
impl From<bincode::error::EncodeError> for Error {
    fn from(err: bincode::error::EncodeError) -> Self {
        Self::Serialization(err.to_string())
    }
}

impl From<bincode::error::DecodeError> for Error {
    fn from(err: bincode::error::DecodeError) -> Self {
        Self::Serialization(err.to_string())
    }
}

impl Error {
    /// A stable discriminant for metrics / structured logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RocksDb(_) => "rocksdb",
            #[cfg(feature = "surrealdb-storage")]
            Self::SurrealDb(_) => "surrealdb",
            Self::Serialization(_) => "serialization",
            Self::NotFound { .. } => "not_found",
            Self::Format(_) => "format",
            Self::Io(_) => "io",
            Self::External(_) => "external",
        }
    }
}
