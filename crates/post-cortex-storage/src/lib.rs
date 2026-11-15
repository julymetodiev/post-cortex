// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Storage backends for post-cortex.
//!
//! The adapters of the storage ports of `post-cortex-ports`: the RocksDB
//! backend ([`RealRocksDBStorage`]), an optional SurrealDB backend
//! ([`SurrealDBStorage`], behind the `surrealdb-storage` feature), their
//! on-disk rows and migrations, and the `ArchiveCodec` adapter of the
//! `pcx export` file format ([`export_import`]). Domain types come from `post-cortex-core`, the
//! ports and the types they speak from `post-cortex-ports`.
//!
//! ```text
//! use post_cortex_ports::Stores;
//! use post_cortex_storage::RealRocksDBStorage;
//! ```

#![forbid(unsafe_code)]

pub mod backend;
mod breaker;
mod document_order;
pub mod export_import;
pub mod freshness;
mod message_blocks;
mod port_error;
pub mod rocksdb_storage;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod document_speed_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod workspace_project_tests;

#[cfg(feature = "surrealdb-storage")]
pub mod surrealdb_storage;

pub use backend::{StorageBackendType, UnknownStorageBackend};
pub use export_import::{ArchiveError, JsonArchive, check_format_version};
pub use rocksdb_storage::RealRocksDBStorage;

#[cfg(feature = "surrealdb-storage")]
pub use surrealdb_storage::SurrealDBStorage;
