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
// Storage records (StoredEntity, ExportData, etc.) are large by design —
// they're persisted records, not transient values. Boxing them on the
// Err path would obscure the API.
#![allow(clippy::result_large_err)]
// Some persisted records have many fields; refactoring into nested
// helper types would change the on-disk serialization layout.
#![allow(clippy::type_complexity)]
// Construction patterns like `let mut cfg = StorageConfig::default(); cfg.x = y;`
// are clearer than a struct literal with `..Default::default()` in the
// large config types here.
#![allow(clippy::field_reassign_with_default)]
// `from_str` here pre-dates `std::str::FromStr` adoption; full FromStr
// impls land during the Phase 9 error-typing follow-up.
#![allow(clippy::should_implement_trait)]
// `format!` inside a format arg is occasionally clearer than computing
// the inner string ahead of time.
#![allow(clippy::format_in_format_args)]

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
