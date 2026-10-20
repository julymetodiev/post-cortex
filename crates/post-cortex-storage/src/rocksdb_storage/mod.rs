// Copyright (c) 2025, 2026 Julius ML
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

//! RocksDB storage backend split by trait concern.
//!
//! Public surface preserved through re-exports below — external code imports
//! `RealRocksDBStorage`, `SessionCheckpoint`, etc. from `crate::rocksdb_storage`
//! exactly as before.

use rocksdb::DB;
use std::path::PathBuf;
use std::sync::Arc;

mod core;
mod freshness;
mod graph;
mod messages;
mod sessions;
mod types;
mod vectors;
mod workspaces;

#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;

pub use types::{
    EMBEDDING_DIMENSION, SessionCheckpoint, StoredEmbedding, StoredEntity, StoredRelationship,
    StoredWorkspace, StoredWorkspaceSession,
};

/// Real RocksDB storage implementation for high performance.
///
/// All records share one key space (key families are documented on the
/// module that owns them). Vector search is served by the memory layer's
/// own in-memory index; this backend only persists embeddings, and its
/// [`VectorStorage`](crate::traits::VectorStorage) search methods scan the
/// persisted rows.
#[derive(Clone)]
pub struct RealRocksDBStorage {
    pub(super) db: Arc<DB>,
    #[allow(dead_code)]
    pub(super) data_dir: PathBuf,
    /// Required embedding length; `None` accepts any non-empty vector.
    pub(super) vector_dimension: Option<usize>,
    /// Serialises read-modify-write of a session's update index.
    pub(super) session_locks: Arc<core::SessionLocks>,
}
