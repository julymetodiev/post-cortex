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

//! Persisted record types and shared constants for the RocksDB backend.

use chrono::{DateTime, Utc};
use post_cortex_ports::VectorMetadata;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Output dimension of the legacy MultilingualMiniLM model (384).
///
/// No longer enforced by default: [`RealRocksDBStorage::new`] accepts any
/// non-empty vector, and [`RealRocksDBStorage::new_with_dimension`] enforces
/// the configured model's dimension.
///
/// [`RealRocksDBStorage::new`]: crate::rocksdb_storage::RealRocksDBStorage::new
/// [`RealRocksDBStorage::new_with_dimension`]: crate::rocksdb_storage::RealRocksDBStorage::new_with_dimension
pub const EMBEDDING_DIMENSION: usize = 384;

/// Stored embedding record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredEmbedding {
    /// Stable content ID this embedding is keyed by.
    pub content_id: String,
    /// Session that produced the embedding.
    pub session_id: String,
    /// Embedding vector.
    pub vector: Vec<f32>,
    /// Source text that was embedded.
    pub text: String,
    /// Content type tag (e.g. `"message"`, `"summary"`).
    pub content_type: String,
    /// Timestamp of the embedded content.
    pub timestamp: DateTime<Utc>,
    /// Free-form key/value metadata attached to the embedding.
    pub metadata: HashMap<String, String>,
}

impl StoredEmbedding {
    /// Create from vector and metadata
    pub fn new(vector: Vec<f32>, metadata: VectorMetadata) -> Self {
        Self {
            content_id: metadata.id,
            session_id: metadata.source,
            vector,
            text: metadata.text.to_string(),
            content_type: metadata.content_type,
            timestamp: metadata.timestamp,
            metadata: metadata.metadata,
        }
    }

    /// Convert to VectorMetadata
    pub fn to_metadata(&self) -> VectorMetadata {
        VectorMetadata {
            id: self.content_id.clone(),
            text: self.text.as_str().into(),
            source: self.session_id.clone(),
            content_type: self.content_type.clone(),
            timestamp: self.timestamp,
            metadata: self.metadata.clone(),
        }
    }
}
