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
//! - HNSW vector indexing for embeddings
//! - Efficient session and workspace management
//!
//! Enable with: `cargo build --features surrealdb-storage`
//!
//! Split by trait concern so the public surface stays identical — external
//! code imports `SurrealDBStorage` from `crate::storage::surrealdb_storage`
//! exactly as before.

use std::sync::Arc;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

mod core;
mod freshness;
mod graph;
mod import_export;
mod records;
mod sessions;
mod vectors;

#[cfg(test)]
mod tests;

/// Embedding dimension (must match the embedding model)
pub(super) const EMBEDDING_DIMENSION: usize = 384;

/// SurrealDB storage implementation supporting both local (RocksDB) and remote (WebSocket) backends
#[derive(Clone)]
pub struct SurrealDBStorage {
    pub(super) db: Arc<Surreal<Any>>,
    #[allow(dead_code)]
    pub(super) namespace: String,
    #[allow(dead_code)]
    pub(super) database: String,
}
