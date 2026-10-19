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

//! Vector database configuration and search quality presets.

use std::path::PathBuf;

/// Search mode for vector similarity search
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Exact search using full linear scan - highest accuracy, slowest
    Exact,
    /// Approximate search using HNSW - fastest, good accuracy
    Approximate,
    /// Balanced search with optimized HNSW parameters - good speed/accuracy tradeoff
    #[default]
    Balanced,
}

/// Search quality preset for automatic parameter tuning
#[derive(Debug, Clone, Copy)]
pub enum SearchQualityPreset {
    /// Fast search with lower accuracy (ef_search = 32)
    Fast,
    /// Balanced search (ef_search = 64)
    Balanced,
    /// Accurate search with higher recall (ef_search = 128)
    Accurate,
    /// Maximum accuracy search (ef_search = 256)
    Maximum,
}

impl SearchQualityPreset {
    /// Get ef_search value for this preset
    pub fn ef_search(&self) -> usize {
        match self {
            SearchQualityPreset::Fast => 32,
            SearchQualityPreset::Balanced => 64,
            SearchQualityPreset::Accurate => 128,
            SearchQualityPreset::Maximum => 256,
        }
    }
}

/// Configuration for the vector database
#[derive(Debug, Clone)]
pub struct VectorDbConfig {
    /// Vector dimension (must match embedding model)
    pub dimension: usize,
    /// Maximum number of connections per node in HNSW
    pub max_connections: usize,
    /// Size of the dynamic candidate list
    pub ef_construction: usize,
    /// Size of the search candidate list
    pub ef_search: usize,
    /// Number of layers in HNSW hierarchy
    pub num_layers: usize,
    /// Enable vector quantization for memory optimization
    pub enable_quantization: bool,
    /// Quantization buckets (power of 2)
    pub quantization_buckets: usize,
    /// Enable HNSW indexing
    pub enable_hnsw_index: bool,
    /// Up to this many vectors, searches are exact linear scans and no HNSW
    /// graph is maintained (inserts stay O(1)). The first insert that
    /// crosses it builds the graph once; from then on it is kept up to date
    /// incrementally. [`VectorDB::build_index`](super::VectorDB::build_index)
    /// builds it explicitly regardless of size.
    pub exact_search_threshold: usize,
    /// Distance threshold for considering vectors similar
    pub distance_threshold: f32,
    /// Maximum number of vectors to store in total (all sources). Inserts
    /// beyond it fail with an error; bulk inserts stop and warn.
    pub max_vectors: usize,
    /// Auto-save interval (number of operations)
    pub auto_save_interval: usize,
    /// Enable persistent storage
    pub persistent_storage: bool,
    /// Storage path
    pub storage_path: Option<PathBuf>,
    /// Enable Product Quantization for memory optimization
    pub enable_product_quantization: bool,
    /// Number of subvectors for PQ (must divide dimension evenly)
    pub pq_subvectors: usize,
    /// Bits per PQ code (2^bits centroids per subvector)
    pub pq_bits: usize,
    /// Keep in-memory BM25 postings over the vectors' text
    /// ([`VectorDB::search_text`](super::VectorDB::search_text)), for storage
    /// backends without full-text search of their own. Word frequencies for
    /// [`VectorDB::query_terms`](super::VectorDB::query_terms) are kept
    /// regardless.
    pub full_text_index: bool,
}

impl Default for VectorDbConfig {
    fn default() -> Self {
        Self {
            dimension: 384, // Default for MiniLM model
            max_connections: 16,
            ef_construction: 200,
            ef_search: 50,
            num_layers: 4,
            // Off: the u8 copy is never read by search; keep it opt-in.
            enable_quantization: false,
            quantization_buckets: 256,
            enable_hnsw_index: true,
            exact_search_threshold: 10_000,
            distance_threshold: 0.7,
            max_vectors: 100_000,
            auto_save_interval: 1000,
            persistent_storage: false,
            storage_path: None,
            enable_product_quantization: false, // Disabled by default (experimental)
            pq_subvectors: 8,                   // 384/8 = 48 dims per subvector
            pq_bits: 8,                         // 256 centroids per subvector
            full_text_index: false,
        }
    }
}
