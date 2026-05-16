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

//! Public types: stored vectors, vector metadata, search matches, and database statistics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// A stored vector with optional quantization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredVector {
    /// Unique identifier
    pub id: u32,
    /// Original vector
    pub vector: Vec<f32>,
    /// Quantized vector (if quantization is enabled)
    pub quantized: Option<Vec<u8>>,
    /// Product Quantization codes (if PQ is enabled)
    pub pq_codes: Option<Vec<u8>>,
    /// Vector magnitude (precomputed for cosine similarity)
    pub magnitude: f32,
}

impl StoredVector {
    pub(super) fn new(
        id: u32,
        vector: Vec<f32>,
        quantized: Option<Vec<u8>>,
        pq_codes: Option<Vec<u8>>,
    ) -> Self {
        let magnitude = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        Self {
            id,
            vector,
            quantized,
            pq_codes,
            magnitude,
        }
    }
}

/// Metadata for a vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMetadata {
    /// Unique identifier for the vector
    pub id: String,
    /// Original text content
    pub text: String,
    /// Source identifier (e.g., session_id, update_id)
    pub source: String,
    /// Content type classification
    pub content_type: String,
    /// Timestamp when vector was added
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Additional custom metadata
    pub metadata: HashMap<String, String>,
}

impl VectorMetadata {
    /// Create new metadata with required fields
    pub fn new(id: String, text: String, source: String, content_type: String) -> Self {
        Self {
            id,
            text,
            source,
            content_type,
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Add custom metadata
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

/// Search result from vector database
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// Vector ID
    pub vector_id: u32,
    /// Similarity score (cosine similarity)
    pub similarity: f32,
    /// Associated metadata
    pub metadata: VectorMetadata,
}

/// Vector database statistics using atomics
#[derive(Debug, Default)]
pub struct VectorDbStats {
    /// Total number of vectors stored (atomic)
    pub total_vectors: AtomicUsize,
    /// Index construction status (atomic)
    pub is_built: AtomicBool,
    /// Memory usage in bytes (approximate, atomic)
    pub memory_usage_bytes: AtomicUsize,
    /// Total search operations (atomic)
    pub total_searches: AtomicU64,
    /// Total search time in microseconds (atomic)
    pub total_search_time_us: AtomicU64,
    /// Hit rate for recent searches (computed)
    pub search_hit_rate: f64,
    /// Index efficiency metric (connections per node, computed)
    pub index_efficiency: f64,
    /// Quantization compression ratio (computed)
    pub quantization_ratio: f64,
}

impl VectorDbStats {
    /// Create new statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a vector addition
    pub fn record_vector_added(&self, vector_size_bytes: usize) {
        self.total_vectors.fetch_add(1, Ordering::Relaxed);
        self.memory_usage_bytes
            .fetch_add(vector_size_bytes, Ordering::Relaxed);
    }

    /// Record a vector removal with saturating subtraction to prevent underflow
    pub fn record_vector_removed(&self, vector_size_bytes: usize) {
        // Use fetch_update with saturating_sub to prevent underflow on double-remove
        let _ = self
            .total_vectors
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
        let _ = self
            .memory_usage_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(vector_size_bytes))
            });
    }

    /// Record a search operation
    pub fn record_search(&self, duration_us: u64) {
        self.total_searches.fetch_add(1, Ordering::Relaxed);
        self.total_search_time_us
            .fetch_add(duration_us, Ordering::Relaxed);
    }

    /// Get average search time in microseconds
    pub fn avg_search_time_us(&self) -> f64 {
        let total_searches = self.total_searches.load(Ordering::Relaxed);
        let total_time = self.total_search_time_us.load(Ordering::Relaxed);

        if total_searches > 0 {
            total_time as f64 / total_searches as f64
        } else {
            0.0
        }
    }

    /// Get snapshot of current stats
    pub fn snapshot(&self) -> VectorDbStatsSnapshot {
        VectorDbStatsSnapshot {
            total_vectors: self.total_vectors.load(Ordering::Relaxed),
            is_built: self.is_built.load(Ordering::Relaxed),
            memory_usage_bytes: self.memory_usage_bytes.load(Ordering::Relaxed),
            avg_search_time_us: self.avg_search_time_us(),
            search_hit_rate: self.search_hit_rate,
            index_efficiency: self.index_efficiency,
            quantization_ratio: self.quantization_ratio,
        }
    }
}

/// Snapshot of vector database statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorDbStatsSnapshot {
    pub total_vectors: usize,
    pub is_built: bool,
    pub memory_usage_bytes: usize,
    pub avg_search_time_us: f64,
    pub search_hit_rate: f64,
    pub index_efficiency: f64,
    pub quantization_ratio: f64,
}
