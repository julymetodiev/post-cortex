// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Storage backends for post-cortex.
//!
//! Provides the [`Storage`] trait + the lock-free RocksDB backend
//! ([`RealRocksDBStorage`]) and an optional SurrealDB backend
//! ([`SurrealDBStorage`], behind the `surrealdb-backend` feature).
//! Storage is the API of persistence — domain types come from
//! `post-cortex-core`, vector data types from `post-cortex-embeddings`
//! (with `default-features = false` so the ML stack is not pulled in).
//!
//! ```text
//! use post_cortex_storage::{Storage, RealRocksDBStorage};
//! ```

#![forbid(unsafe_code)]

pub mod error;
pub mod export_import;
pub mod rocksdb_storage;
pub mod traits;

pub use error::{Error as StorageError, Result as StorageResult};

#[cfg(feature = "surrealdb-storage")]
pub mod surrealdb_storage;

pub use export_import::{
    CompressionType, ExportData, ExportMetadata, ExportOptions, ExportStats, ExportType,
    ExportedSession, ExportedWorkspace, ImportOptions, ImportResult, list_export_sessions,
    preview_export_file, read_export_file, write_export_file,
};
pub use rocksdb_storage::{RealRocksDBStorage, SessionCheckpoint};
pub use traits::{
    GraphStorage, Storage, StorageBackend, StorageBackendType, StorageConfig, VectorStorage,
};

#[cfg(feature = "surrealdb-storage")]
pub use surrealdb_storage::SurrealDBStorage;
#[cfg(feature = "surrealdb-storage")]
pub use traits::SurrealDBConfig;
