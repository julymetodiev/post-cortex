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

//! RocksDB storage backend: one struct implementing every storage port
//! (see [`post_cortex_ports::storage`]), split into a module per port.
//!
//! Public surface preserved through re-exports below — external code imports
//! `RealRocksDBStorage`, `SessionCheckpoint`, etc. from `crate::rocksdb_storage`
//! exactly as before.

use rocksdb::DB;
use std::sync::Arc;

mod cards;
mod checkpoints;
mod core;
mod dismissals;
mod document_index;
mod documents;
mod events;
mod freshness;
mod graph_rows;
mod legacy;
mod message_rows;
mod messages;
mod migrate;
mod rows;
mod session_listing;
mod session_rows;
mod sessions;
mod source_rows;
mod types;
mod v2_layout;
mod vectors;
mod workspaces;

#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;

pub use types::{EMBEDDING_DIMENSION, StoredEmbedding};

/// Real RocksDB storage implementation for high performance.
///
/// All records share one key space (key families are documented on the
/// module that owns them). Vector search is served by the memory layer's
/// own in-memory index; this backend only persists embeddings (the
/// [`VectorStore`](post_cortex_ports::storage::VectorStore) port).
#[derive(Clone)]
pub struct RealRocksDBStorage {
    db: Arc<DB>,
    /// Required embedding length; `None` accepts any non-empty vector.
    vector_dimension: Option<usize>,
    /// Serialises the saves (and other writes) of one session.
    session_locks: Arc<core::SessionLocks>,
    /// Serialises the bus's appends and cursor moves (`events`).
    bus_lock: Arc<parking_lot::Mutex<()>>,
    /// Refuses calls while the database keeps failing.
    breaker: Arc<crate::breaker::Breaker>,
}
