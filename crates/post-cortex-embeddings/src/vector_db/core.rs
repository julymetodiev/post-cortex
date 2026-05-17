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

//! `VectorDB` — the orchestrator type binding storage, HNSW, quantization, and search.

use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::{debug, info, warn};

use super::common::SearchResult;
use super::config::{SearchMode, SearchQualityPreset, VectorDbConfig};
use super::hnsw_index::HnswIndex;
use super::product_quantization::ProductQuantizationCodebook;
use super::types::{
    SearchMatch, StoredVector, VectorDbStats, VectorDbStatsSnapshot, VectorMetadata,
};

// Type aliases to reduce complexity
type QuantizationParams = Arc<arc_swap::ArcSwap<Option<(Vec<f32>, Vec<f32>)>>>;

/// Vector database using DashMap and atomic operations
pub struct VectorDB {
    /// Vector storage - concurrent DashMap
    vectors: Arc<DashMap<u32, StoredVector>>,
    /// Metadata storage - concurrent DashMap
    metadata: Arc<DashMap<u32, VectorMetadata>>,
    /// Configuration
    config: VectorDbConfig,
    /// Next available vector ID - atomic
    next_id: Arc<AtomicU32>,
    /// Performance statistics using atomics
    stats: Arc<VectorDbStats>,
    /// HNSW index
    hnsw_index: Arc<HnswIndex>,
    /// Vector quantization parameters using ArcSwap
    quantization_params: QuantizationParams,
    /// Product Quantization codebook (if enabled)
    pq_codebook: Option<Arc<ProductQuantizationCodebook>>,
}

impl VectorDB {
    /// Compatibility method to add vector with old-style metadata
    pub fn add_vector_compat(
        &self,
        vector: Vec<f32>,
        content_id: String,
        text: String,
        source: String,
        content_type: String,
    ) -> Result<u32> {
        let metadata = VectorMetadata::new(content_id, text, source, content_type);
        self.add_vector(vector, metadata)
    }

    /// Create a new vector database with the specified configuration
    pub fn new(config: VectorDbConfig) -> Result<Self> {
        info!(
            "Initializing Vector Database with dimension: {}, max_connections: {}, quantization: {}, HNSW: {}, PQ: {}",
            config.dimension,
            config.max_connections,
            config.enable_quantization,
            config.enable_hnsw_index,
            config.enable_product_quantization
        );

        let stats = Arc::new(VectorDbStats::new());
        let quantization_params = Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(None)));

        // Initialize Product Quantization codebook if enabled
        let pq_codebook = if config.enable_product_quantization {
            info!(
                "Initializing PQ codebook: {} subvectors, {} bits",
                config.pq_subvectors, config.pq_bits
            );
            Some(Arc::new(ProductQuantizationCodebook::new(
                config.dimension,
                config.pq_subvectors,
                config.pq_bits,
            )?))
        } else {
            None
        };

        Ok(Self {
            vectors: Arc::new(DashMap::new()),
            metadata: Arc::new(DashMap::new()),
            config,
            next_id: Arc::new(AtomicU32::new(0)),
            stats,
            hnsw_index: Arc::new(HnswIndex::new()),
            quantization_params,
            pq_codebook,
        })
    }

    /// Add a vector to the database
    pub fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<u32> {
        if vector.len() != self.config.dimension {
            return Err(anyhow::anyhow!(
                "Vector dimension {} does not match expected {}",
                vector.len(),
                self.config.dimension
            ));
        }

        // Atomic ID generation
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        // Quantize vector if enabled
        let quantized = if self.config.enable_quantization {
            self.quantize_vector(&vector).ok()
        } else {
            None
        };

        // Product Quantization encoding if enabled
        let pq_codes = self
            .pq_codebook
            .as_ref()
            .map(|codebook| codebook.encode(&vector));

        // Create stored vector
        let stored_vector = StoredVector::new(id, vector, quantized, pq_codes);
        let vector_size_bytes = stored_vector_size(&stored_vector);

        // Add to vectors collection
        self.vectors.insert(id, stored_vector);

        // Add metadata
        self.metadata.insert(id, metadata);

        // Update statistics
        self.stats.record_vector_added(vector_size_bytes);

        // Build HNSW index if enabled
        if self.config.enable_hnsw_index {
            self.build_hnsw_index_for_vector(id)?;
        }

        debug!("Added vector {} to database", id);
        Ok(id)
    }

    /// Search for similar vectors
    pub fn search(&self, query_vector: &[f32], k: usize) -> Result<Vec<SearchMatch>> {
        // Use exact (linear) scan for small-to-medium DBs — HNSW post-filter
        // misses sparse clusters and is only worth it for very large DBs.
        let mode = if self.vectors.len() <= 10_000 {
            SearchMode::Exact
        } else {
            SearchMode::default()
        };
        self.search_with_mode(query_vector, k, mode, None)
    }

    /// Search for similar vectors with configurable mode and parameters
    ///
    /// # Arguments
    /// * `query_vector` - Query vector to search for
    /// * `k` - Number of results to return
    /// * `mode` - Search mode (Exact, Approximate, Balanced)
    /// * `ef_search_override` - Optional override for ef_search parameter (if None, uses mode default)
    pub fn search_with_mode(
        &self,
        query_vector: &[f32],
        k: usize,
        mode: SearchMode,
        ef_search_override: Option<usize>,
    ) -> Result<Vec<SearchMatch>> {
        if query_vector.len() != self.config.dimension {
            return Err(anyhow::anyhow!(
                "Query vector dimension {} does not match expected {}",
                query_vector.len(),
                self.config.dimension
            ));
        }

        let start_time = std::time::Instant::now();

        // Select search strategy based on mode
        let results = match mode {
            SearchMode::Exact => {
                // Always use linear search for exact mode
                debug!("Using exact search (linear scan)");
                self.linear_search(query_vector, k)?
            }
            SearchMode::Approximate | SearchMode::Balanced => {
                // Use HNSW if available, otherwise fall back to linear
                if self.config.enable_hnsw_index && !self.hnsw_index.is_empty() {
                    let ef_search = ef_search_override.unwrap_or_else(|| match mode {
                        SearchMode::Approximate => SearchQualityPreset::Fast.ef_search(),
                        SearchMode::Balanced => SearchQualityPreset::Balanced.ef_search(),
                        _ => self.config.ef_search,
                    });
                    debug!("Using HNSW search with ef_search={}", ef_search);
                    self.hnsw_search_with_ef(query_vector, k, ef_search)?
                } else {
                    debug!("HNSW index not available, falling back to linear search");
                    self.linear_search(query_vector, k)?
                }
            }
        };

        // Metadata access via DashMap
        let mut matches = Vec::new();

        for result in results {
            let vector_id = result.id;

            if let Some(metadata) = self.metadata.get(&vector_id) {
                matches.push(SearchMatch {
                    vector_id,
                    similarity: result.similarity,
                    metadata: metadata.clone(),
                });
            }
        }

        // Update statistics
        let duration_us = start_time.elapsed().as_micros() as u64;
        self.stats.record_search(duration_us);

        debug!(
            "Search completed in {}μs with mode {:?}, found {} matches",
            duration_us,
            mode,
            matches.len()
        );
        Ok(matches)
    }

    /// Linear search through all vectors
    fn linear_search(&self, query_vector: &[f32], k: usize) -> Result<Vec<SearchResult>> {
        let mut similarities: Vec<_> = Vec::new();

        // Iterate over vectors
        for entry in self.vectors.iter() {
            let stored_vector = entry.value();
            let similarity = Self::calculate_cosine_similarity(query_vector, &stored_vector.vector);
            similarities.push(SearchResult {
                id: stored_vector.id,
                similarity,
            });
        }

        // Sort by similarity (descending)
        similarities.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        similarities.truncate(k);

        Ok(similarities)
    }

    /// HNSW search with configurable ef_search parameter
    ///
    /// Uses BinaryHeap for O(log n) candidate management instead of O(n log n) Vec+sort.
    ///
    /// # Arguments
    /// * `query_vector` - Query vector to search for
    /// * `k` - Number of results to return
    /// * `ef_search` - Size of the dynamic candidate list (higher = more accurate but slower)
    fn hnsw_search_with_ef(
        &self,
        query_vector: &[f32],
        k: usize,
        ef_search: usize,
    ) -> Result<Vec<SearchResult>> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        // Get entry point
        let entry_point = self
            .hnsw_index
            .get_entry_point()
            .ok_or_else(|| anyhow::anyhow!("HNSW index is empty"))?;

        // Max-heap for candidates (best similarity first via Ord impl)
        let mut candidates: BinaryHeap<SearchResult> = BinaryHeap::new();
        // Min-heap for results (worst similarity first, to efficiently drop worst)
        let mut results: BinaryHeap<Reverse<SearchResult>> = BinaryHeap::new();
        let mut visited = std::collections::HashSet::new();

        // Start from entry point
        let initial_similarity = self.get_vector_similarity(query_vector, entry_point)?;
        candidates.push(SearchResult {
            id: entry_point,
            similarity: initial_similarity,
        });
        results.push(Reverse(SearchResult {
            id: entry_point,
            similarity: initial_similarity,
        }));
        visited.insert(entry_point);

        // Expand search through connections using heap
        while let Some(current) = candidates.pop() {
            // Early termination: if current candidate is worse than worst result, stop
            if results.len() >= ef_search {
                if let Some(Reverse(worst)) = results.peek() {
                    if current.similarity < worst.similarity {
                        break;
                    }
                }
            }

            // Explore connections
            if let Some(connections) = self.hnsw_index.get_connections(current.id) {
                for &connected_id in &connections {
                    if visited.insert(connected_id) {
                        let similarity = self.get_vector_similarity(query_vector, connected_id)?;
                        let result = SearchResult {
                            id: connected_id,
                            similarity,
                        };

                        // Add to candidates heap
                        candidates.push(result.clone());

                        // Add to results min-heap
                        results.push(Reverse(result));

                        // Keep only top ef_search results
                        while results.len() > ef_search {
                            results.pop(); // Removes worst (smallest similarity)
                        }
                    }
                }
            }
        }

        // Extract top k from results and sort by similarity (descending)
        let mut final_results: Vec<SearchResult> = results.into_iter().map(|r| r.0).collect();
        final_results.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        final_results.truncate(k);

        Ok(final_results)
    }

    /// Get vector similarity (helper for HNSW search)
    fn get_vector_similarity(&self, query_vector: &[f32], vector_id: u32) -> Result<f32> {
        self.vectors
            .get(&vector_id)
            .map(|entry| {
                let stored_vector = entry.value();
                Self::calculate_cosine_similarity(query_vector, &stored_vector.vector)
            })
            .ok_or_else(|| anyhow::anyhow!("Vector {} not found", vector_id))
    }

    /// Calculate cosine similarity between two vectors
    fn calculate_cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot_product / (norm_a * norm_b)
        }
    }

    /// Quantize a vector (simplified implementation)
    fn quantize_vector(&self, vector: &[f32]) -> Result<Vec<u8>> {
        let params_guard = self.quantization_params.load();
        let params = params_guard.as_ref();

        if let Some((min_vals, max_vals)) = params {
            let mut quantized = Vec::with_capacity(vector.len());

            for (i, &value) in vector.iter().enumerate() {
                if i < min_vals.len() && i < max_vals.len() {
                    let min_val = min_vals[i];
                    let max_val = max_vals[i];

                    let bucket = if max_val > min_val {
                        let normalized = (value - min_val) / (max_val - min_val);
                        (normalized * (self.config.quantization_buckets - 1) as f32) as u8
                    } else {
                        // Edge case: all values are identical - map to middle bucket
                        // This is more semantically meaningful than mapping to 0
                        (self.config.quantization_buckets / 2) as u8
                    };
                    quantized.push(bucket.min(self.config.quantization_buckets as u8 - 1));
                } else {
                    quantized.push(0);
                }
            }

            Ok(quantized)
        } else {
            // No quantization parameters available, return raw bytes
            // Map typical embedding range [-1.0, 1.0] to [0, 255]
            Ok(vector
                .iter()
                .map(|&x| ((x + 1.0) * 127.5).clamp(0.0, 255.0) as u8)
                .collect())
        }
    }

    /// Assign a random layer to a new vector using exponential distribution
    /// Higher layers are exponentially rarer, creating the hierarchical structure
    fn random_layer(&self) -> usize {
        // ml = 1 / ln(M) where M is max_connections
        // This gives the optimal layer distribution for HNSW
        let ml = 1.0 / (self.config.max_connections as f64).ln();
        let r: f64 = rand::random::<f64>().max(1e-10); // Avoid log(0)
        let layer = (-r.ln() * ml).floor() as usize;
        // Cap at num_layers to prevent unbounded growth
        layer.min(self.config.num_layers.saturating_sub(1))
    }

    /// Find k nearest neighbors to a query vector from existing vectors
    fn find_nearest_neighbors(
        &self,
        query_vector: &[f32],
        k: usize,
        exclude_id: Option<u32>,
    ) -> Vec<SearchResult> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        // Min-heap to keep track of k best (worst at top for easy removal)
        let mut heap: BinaryHeap<Reverse<SearchResult>> = BinaryHeap::new();

        for entry in self.vectors.iter() {
            let id = *entry.key();

            // Skip excluded vector (self)
            if Some(id) == exclude_id {
                continue;
            }

            let stored = entry.value();
            let similarity = Self::calculate_cosine_similarity(query_vector, &stored.vector);
            let result = SearchResult { id, similarity };

            if heap.len() < k {
                heap.push(Reverse(result));
            } else if let Some(Reverse(worst)) = heap.peek() {
                if similarity > worst.similarity {
                    heap.pop();
                    heap.push(Reverse(result));
                }
            }
        }

        // Extract results sorted by similarity (descending)
        let mut results: Vec<_> = heap.into_iter().map(|r| r.0).collect();
        results.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    /// Add bidirectional connection between two vectors in the HNSW index
    fn add_bidirectional_connection(&self, from_id: u32, to_id: u32) {
        // Add connection from -> to
        self.hnsw_index
            .connections
            .entry(from_id)
            .or_default()
            .push(to_id);

        // Add connection to -> from
        self.hnsw_index
            .connections
            .entry(to_id)
            .or_default()
            .push(from_id);
    }

    /// Prune connections to keep only the best max_connections neighbors
    fn prune_connections(&self, vector_id: u32, max_connections: usize) {
        if let Some(mut entry) = self.hnsw_index.connections.get_mut(&vector_id) {
            let connections = entry.value_mut();
            if connections.len() <= max_connections {
                return;
            }

            // Get the vector we're pruning connections for
            let query = match self.vectors.get(&vector_id) {
                Some(v) => v.vector.clone(),
                None => return,
            };

            // Calculate similarities and keep only the best connections
            let mut scored: Vec<_> = connections
                .iter()
                .filter_map(|&neighbor_id| {
                    self.vectors.get(&neighbor_id).map(|v| {
                        let sim = Self::calculate_cosine_similarity(&query, &v.vector);
                        (neighbor_id, sim)
                    })
                })
                .collect();

            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            *connections = scored
                .into_iter()
                .take(max_connections)
                .map(|(id, _)| id)
                .collect();
        }
    }

    /// Build HNSW index for a single vector using proper nearest neighbor selection
    ///
    /// This implements a simplified but correct HNSW insertion:
    /// 1. Assigns a random layer using exponential distribution
    /// 2. Finds actual nearest neighbors (not random vectors)
    /// 3. Creates bidirectional connections
    /// 4. Prunes connections to maintain graph sparsity
    fn build_hnsw_index_for_vector(&self, vector_id: u32) -> Result<()> {
        let max_connections = self.config.max_connections;

        // Get the vector being indexed
        let query_vector = self
            .vectors
            .get(&vector_id)
            .map(|v| v.vector.clone())
            .ok_or_else(|| anyhow::anyhow!("Vector {} not found", vector_id))?;

        // Assign random layer (exponential distribution)
        let layer = self.random_layer();

        // First vector - just add it with no connections
        if self.hnsw_index.is_empty() {
            self.hnsw_index.add_vector(vector_id, layer, Vec::new());
            debug!(
                "Built HNSW index for first vector {} at layer {}",
                vector_id, layer
            );
            return Ok(());
        }

        // Find nearest neighbors using actual distance calculation
        let ef_construction = self.config.ef_construction.max(max_connections * 2);
        let neighbors =
            self.find_nearest_neighbors(&query_vector, ef_construction, Some(vector_id));

        // Select best neighbors (up to max_connections)
        let selected_neighbors: Vec<u32> = neighbors
            .iter()
            .take(max_connections)
            .map(|r| r.id)
            .collect();

        // Add bidirectional connections
        for &neighbor_id in &selected_neighbors {
            self.add_bidirectional_connection(vector_id, neighbor_id);
            // Prune neighbor's connections if they have too many
            self.prune_connections(neighbor_id, max_connections);
        }

        // Add vector to index
        self.hnsw_index
            .add_vector(vector_id, layer, selected_neighbors.clone());

        debug!(
            "Built HNSW index for vector {} at layer {} with {} neighbors (nearest similarity: {:.3})",
            vector_id,
            layer,
            selected_neighbors.len(),
            neighbors.first().map(|r| r.similarity).unwrap_or(0.0)
        );
        Ok(())
    }

    /// Get metadata for a vector
    pub fn get_metadata(&self, vector_id: u32) -> Option<VectorMetadata> {
        self.metadata.get(&vector_id).map(|entry| entry.clone())
    }

    /// Remove a vector from the database
    pub fn remove_vector(&self, vector_id: u32) -> Result<bool> {
        // Remove from vectors
        let removed_vector = self.vectors.remove(&vector_id);

        // Remove metadata - we don't need the removed value
        self.metadata.remove(&vector_id);

        if let Some(removed) = removed_vector {
            // Update statistics - match size calculation from add_vector
            let vector_size_bytes = stored_vector_size(&removed.1);
            self.stats.record_vector_removed(vector_size_bytes);

            // Remove from HNSW index
            self.hnsw_index.remove_vector(vector_id);

            debug!("Removed vector {} from database", vector_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get database statistics
    pub fn get_stats(&self) -> VectorDbStatsSnapshot {
        self.stats.snapshot()
    }

    /// Get total number of vectors in the database
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Check if the database is empty
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// Check if database has embeddings for a session
    pub fn has_session_embeddings(&self, session_id: &str) -> bool {
        self.metadata
            .iter()
            .any(|entry| entry.value().source == session_id)
    }

    /// Count embeddings for a session
    pub fn count_session_embeddings(&self, session_id: &str) -> usize {
        self.metadata
            .iter()
            .filter(|entry| entry.value().source == session_id)
            .count()
    }

    /// Check if session has update embeddings (not just entities)
    pub fn has_session_update_embeddings(&self, session_id: &str) -> bool {
        self.metadata.iter().any(|entry| {
            entry.value().source == session_id && entry.value().content_type != "EntityDescription"
        })
    }

    /// Check if a specific update_id exists as an embedding
    pub fn has_update_embedding(&self, update_id: &str) -> bool {
        self.metadata
            .iter()
            .any(|entry| entry.value().id == update_id)
    }

    /// Find vector_id by content_id (for removal operations)
    pub fn find_vector_id_by_content_id(&self, content_id: &str) -> Option<u32> {
        self.metadata
            .iter()
            .find(|entry| entry.value().id == content_id)
            .map(|entry| *entry.key())
    }

    /// Get list of update IDs that exist in vector DB for a session
    pub fn get_vectorized_update_ids(&self, session_id: &str) -> Vec<String> {
        self.metadata
            .iter()
            .filter(|entry| {
                entry.value().source == session_id
                    && entry.value().content_type != "EntityDescription"
            })
            .map(|entry| entry.value().id.clone())
            .collect()
    }

    /// Build HNSW index for all vectors
    pub fn build_index(&self) -> Result<()> {
        info!("Building HNSW index for {} vectors", self.vectors.len());

        // Collect keys first to avoid modifying during iteration
        let existing_vector_ids: Vec<u32> = self
            .hnsw_index
            .connections
            .iter()
            .map(|entry| *entry.key())
            .collect();

        // Now safely remove
        for vector_id in existing_vector_ids {
            self.hnsw_index.remove_vector(vector_id);
        }

        // Collect vector IDs to build index for
        let vector_ids: Vec<u32> = self.vectors.iter().map(|entry| *entry.key()).collect();

        // Build index for each vector
        for vector_id in vector_ids {
            self.build_hnsw_index_for_vector(vector_id)?;
        }

        // Mark index as built
        self.stats.is_built.store(true, Ordering::Relaxed);

        info!("HNSW index built successfully");
        Ok(())
    }

    /// Clear all vectors from the database
    pub fn clear(&self) -> Result<()> {
        let vector_count = self.vectors.len();

        // Clear all collections
        self.vectors.clear();
        self.metadata.clear();

        // Collect keys first to avoid modifying during iteration
        let hnsw_vector_ids: Vec<u32> = self
            .hnsw_index
            .connections
            .iter()
            .map(|entry| *entry.key())
            .collect();

        // Clear HNSW index safely
        for vector_id in hnsw_vector_ids {
            self.hnsw_index.remove_vector(vector_id);
        }

        // Reset statistics
        self.stats.total_vectors.store(0, Ordering::Relaxed);
        self.stats.memory_usage_bytes.store(0, Ordering::Relaxed);
        self.stats.is_built.store(false, Ordering::Relaxed);

        // Reset ID counter
        self.next_id.store(0, Ordering::Relaxed);

        info!("Cleared {} vectors from database", vector_count);
        Ok(())
    }

    /// Add vectors in batch
    pub fn add_vectors_batch(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> Result<Vec<u32>> {
        let mut ids = Vec::with_capacity(vectors.len());

        for (vector, metadata) in vectors {
            match self.add_vector(vector, metadata) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    warn!("Failed to add vector to batch: {}", e);
                    // Continue with other vectors
                }
            }
        }

        info!("Added {} vectors in batch", ids.len());
        Ok(ids)
    }

    /// Search with custom filter
    pub fn search_with_filter<F>(
        &self,
        query_vector: &[f32],
        k: usize,
        filter: F,
    ) -> Result<Vec<SearchMatch>>
    where
        F: Fn(&VectorMetadata) -> bool,
    {
        let total_vectors = self.vectors.len();

        // For small databases or when filtering, use exact search to ensure we find all matches
        // This is more accurate for session-scoped queries where filtered results may be sparse
        let (search_limit, search_mode, ef_override) = if total_vectors <= 10_000 {
            // Small database: use exact search for guaranteed accuracy
            debug!(
                "search_with_filter: using Exact mode for small database ({} vectors)",
                total_vectors
            );
            (total_vectors, SearchMode::Exact, None)
        } else {
            // Large database: use HNSW with increased multiplier for filtered queries
            // k*10 ensures enough candidates when filtering by session/content_type
            let search_k = (k * 10).min(total_vectors);
            let dynamic_ef_search = self.config.ef_search.max(search_k * 2);
            debug!(
                "search_with_filter: k={}, search_k={}, dynamic_ef_search={}",
                k, search_k, dynamic_ef_search
            );
            (search_k, SearchMode::Balanced, Some(dynamic_ef_search))
        };

        let all_matches =
            self.search_with_mode(query_vector, search_limit, search_mode, ef_override)?;

        debug!(
            "search_with_filter: got {} results before filtering",
            all_matches.len()
        );

        // Log ContentType distribution for debugging cross-language search issues
        if log::log_enabled!(log::Level::Debug) {
            let mut content_type_counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for m in &all_matches {
                *content_type_counts
                    .entry(m.metadata.content_type.clone())
                    .or_insert(0) += 1;
            }
            debug!(
                "search_with_filter: ContentType distribution: {:?}",
                content_type_counts
            );
        }

        let filtered_matches: Vec<_> = all_matches
            .into_iter()
            .filter(|match_| filter(&match_.metadata))
            .take(k)
            .collect();

        Ok(filtered_matches)
    }

    /// Search in specific source
    pub fn search_in_source(
        &self,
        query_vector: &[f32],
        k: usize,
        source: &str,
    ) -> Result<Vec<SearchMatch>> {
        self.search_with_filter(query_vector, k, |metadata| metadata.source == source)
    }

    /// Search by content type
    pub fn search_by_content_type(
        &self,
        query_vector: &[f32],
        k: usize,
        content_type: &str,
    ) -> Result<Vec<SearchMatch>> {
        self.search_with_filter(query_vector, k, |metadata| {
            metadata.content_type == content_type
        })
    }
}

impl Default for VectorDB {
    fn default() -> Self {
        Self::new(VectorDbConfig::default()).expect("Failed to create default vector database")
    }
}

/// Approximate bytes occupied by a stored vector (used for memory stats).
fn stored_vector_size(stored: &StoredVector) -> usize {
    std::mem::size_of::<StoredVector>()
        + stored.vector.len() * std::mem::size_of::<f32>()
        + stored.quantized.as_ref().map(|q| q.len()).unwrap_or(0)
        + stored.pq_codes.as_ref().map(|pq| pq.len()).unwrap_or(0)
}
