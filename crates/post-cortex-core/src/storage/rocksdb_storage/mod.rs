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
//! `RealRocksDBStorage`, `SessionCheckpoint`, etc. from `crate::storage::rocksdb_storage`
//! exactly as before.

use rocksdb::DB;
use std::path::PathBuf;
use std::sync::Arc;

use crate::core::vector_db::VectorDB;

mod core;
mod freshness;
mod graph;
mod sessions;
mod types;
mod vectors;
mod workspaces;

#[cfg(test)]
mod tests;

pub use types::{
    EMBEDDING_DIMENSION, SessionCheckpoint, StoredEmbedding, StoredEntity, StoredRelationship,
    StoredWorkspace, StoredWorkspaceSession,
};

/// Real RocksDB storage implementation for high performance.
///
/// Uses a hybrid approach:
/// - RocksDB for persistent storage on disk
/// - In-memory VectorDB with HNSW index for O(log n) vector search
#[derive(Clone)]
pub struct RealRocksDBStorage {
    pub(super) db: Arc<DB>,
    #[allow(dead_code)]
    pub(super) data_dir: PathBuf,
    /// In-memory HNSW index for fast vector search
    pub(super) vector_index: Arc<VectorDB>,
}
