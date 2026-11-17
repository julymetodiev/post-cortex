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

//! SurrealDB storage backend for post-cortex.
//!
//! This module provides a SurrealDB-based storage implementation with:
//! - Native graph support for entity relationships
//! - Persisted embeddings (searched by the memory layer's in-memory index)
//! - Efficient session and workspace management
//!
//! Enable with: `cargo build --features surrealdb-storage`
//!
//! One struct implementing every storage port (see [`post_cortex_ports::storage`]),
//! split into a module per port.

use std::sync::Arc;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

mod cards;
mod checkpoints;
mod connection;
mod core;
mod dismissals;
mod document_index;
mod documents;
mod events;
mod freshness;
mod graph;
mod ids;
mod limit;
mod messages;
mod migrate;
mod persist;
mod records;
mod schema;
mod sessions;
mod transaction;
mod vectors;
mod workspaces;

pub use connection::{ANSWER, Connection, MISSES, Opener, PROBE};

#[cfg(test)]
mod corrupt_tests;
#[cfg(test)]
mod normalized_tests;
#[cfg(test)]
mod persist_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vector_tests;

/// Shortest embedding stored without a configured dimension: a sanity floor
/// against malformed payloads.
const MIN_VECTOR_LEN: usize = 8;

/// SurrealDB storage implementation supporting both local (RocksDB) and remote (WebSocket) backends
#[derive(Clone)]
pub struct SurrealDBStorage {
    /// The connection, replaced when it stops answering.
    connection: Arc<Connection>,
    /// `namespace/database`, for messages.
    name: String,
    /// Required embedding length; `None` accepts any of at least
    /// [`MIN_VECTOR_LEN`].
    vector_dimension: Option<usize>,
    /// Refuses calls while the database keeps failing.
    breaker: Arc<crate::breaker::Breaker>,
}

impl SurrealDBStorage {
    /// The connection a call uses now.
    fn db(&self) -> Arc<Surreal<Any>> {
        self.connection.db()
    }
}
