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
//!
//! Search paths:
//! - Up to [`VectorDbConfig::exact_search_threshold`] vectors every search is
//!   an exact scan with a bounded top-k heap, and no HNSW graph is kept.
//! - Above it the graph is built once and maintained incrementally; unfiltered
//!   default searches and generic filtered searches use it.
//! - Source-scoped searches ([`VectorDB::search_in_sources`]) always score
//!   only that source's vectors, found through a secondary index.
//!
//! Locking: no code path holds a guard on one map while writing another, so
//! the DashMaps cannot deadlock against each other.

use anyhow::Result;
use dashmap::DashMap;
use rayon::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tracing::{debug, info, warn};

use super::common::{SearchResult, TopK};
use super::config::{SearchMode, SearchQualityPreset, VectorDbConfig};
use super::hnsw_index::HnswIndex;
use super::product_quantization::ProductQuantizationCodebook;
use super::similarity::{cosine_with_norms, l2_norm};
use super::text_index::TextIndex;
use super::types::{
    SearchMatch, StoredVector, VectorDbStats, VectorDbStatsSnapshot, VectorMetadata,
};

// Type aliases to reduce complexity
type QuantizationParams = Arc<arc_swap::ArcSwap<Option<(Vec<f32>, Vec<f32>)>>>;

/// Content type of entity-description vectors; everything else counts as an
/// update embedding for the per-source update queries.
const ENTITY_DESCRIPTION: &str = "EntityDescription";

/// Ids belonging to one `VectorMetadata::source`.
#[derive(Debug, Default)]
struct SourceIds {
    ids: HashSet<u32>,
    /// How many of `ids` are not `EntityDescription` vectors.
    update_count: usize,
}

/// Vector database using DashMap and atomic operations
pub struct VectorDB {
    /// Vector storage - concurrent DashMap
    vectors: Arc<DashMap<u32, StoredVector>>,
    /// Metadata storage - concurrent DashMap
    metadata: Arc<DashMap<u32, VectorMetadata>>,
    /// Secondary index: `metadata.source` -> vector ids
    by_source: DashMap<String, SourceIds>,
    /// Secondary index: `metadata.id` (content id) -> vector ids, oldest first
    by_content_id: DashMap<String, Vec<u32>>,
    /// Configuration
    config: VectorDbConfig,
    /// Next available vector ID - atomic
    next_id: Arc<AtomicU32>,
    /// Performance statistics using atomics
    stats: Arc<VectorDbStats>,
    /// HNSW index
    hnsw_index: Arc<HnswIndex>,
    /// Whether the HNSW graph is live (built and maintained on insert)
    hnsw_active: AtomicBool,
    /// Serialises graph (re)builds
    build_lock: parking_lot::Mutex<()>,
    /// Vector quantization parameters using ArcSwap
    quantization_params: QuantizationParams,
    /// Product Quantization codebook (if enabled)
    pq_codebook: Option<Arc<ProductQuantizationCodebook>>,
    /// Word document frequencies, and BM25 postings with
    /// `config.full_text_index`
    text_index: TextIndex,
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
            by_source: DashMap::new(),
            by_content_id: DashMap::new(),
            text_index: TextIndex::new(config.full_text_index),
            config,
            next_id: Arc::new(AtomicU32::new(0)),
            stats,
            hnsw_index: Arc::new(HnswIndex::new()),
            hnsw_active: AtomicBool::new(false),
            build_lock: parking_lot::Mutex::new(()),
            quantization_params,
            pq_codebook,
        })
    }

    /// Add a vector to the database
    ///
    /// Below [`VectorDbConfig::exact_search_threshold`] this is O(1); the
    /// insert that crosses the threshold builds the HNSW graph once.
    pub fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<u32> {
        let id = self.insert_unindexed(vector, metadata)?;
        self.index_new_vectors(&[id]);
        debug!("Added vector {} to database", id);
        Ok(id)
    }

    /// Add many vectors, deferring HNSW work until all are stored.
    ///
    /// Use this for startup loads: one graph build at the end instead of an
    /// incremental insert per vector. Vectors with a wrong dimension are
    /// skipped with a warning; once `max_vectors` is reached the rest are
    /// dropped with a warning. Returns the ids of the stored vectors, in
    /// input order.
    pub fn add_vectors_bulk<I>(&self, vectors: I) -> Result<Vec<u32>>
    where
        I: IntoIterator<Item = (Vec<f32>, VectorMetadata)>,
    {
        let iter = vectors.into_iter();
        let mut ids = Vec::with_capacity(iter.size_hint().0);
        let mut dropped_at_capacity = 0usize;

        for (vector, metadata) in iter {
            if dropped_at_capacity > 0 || self.at_capacity() {
                dropped_at_capacity += 1;
                continue;
            }
            match self.insert_unindexed(vector, metadata) {
                Ok(id) => ids.push(id),
                Err(e) => warn!("Failed to add vector to batch: {}", e),
            }
        }

        if dropped_at_capacity > 0 {
            warn!(
                "Vector DB is full (max_vectors = {}): dropped {} vectors from batch",
                self.config.max_vectors, dropped_at_capacity
            );
        }

        self.index_new_vectors(&ids);

        info!("Added {} vectors in batch", ids.len());
        Ok(ids)
    }

    /// Add vectors in batch. Same as [`Self::add_vectors_bulk`].
    pub fn add_vectors_batch(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> Result<Vec<u32>> {
        self.add_vectors_bulk(vectors)
    }

    fn at_capacity(&self) -> bool {
        self.vectors.len() >= self.config.max_vectors
    }

    /// Validate and store a vector plus its metadata and secondary-index
    /// entries, without touching the HNSW graph.
    fn insert_unindexed(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<u32> {
        if vector.len() != self.config.dimension {
            return Err(anyhow::anyhow!(
                "Vector dimension {} does not match expected {}",
                vector.len(),
                self.config.dimension
            ));
        }
        // Checked without a reservation, so concurrent inserts can overshoot
        // the cap by at most the number of racing threads.
        if self.at_capacity() {
            return Err(anyhow::anyhow!(
                "Vector DB is full: max_vectors = {} reached",
                self.config.max_vectors
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

        let stored_vector = StoredVector::new(id, vector, quantized, pq_codes);
        let vector_size_bytes = stored_vector_size(&stored_vector);

        self.index_metadata(id, &metadata);
        self.vectors.insert(id, stored_vector);
        self.metadata.insert(id, metadata);

        self.stats.record_vector_added(vector_size_bytes);
        Ok(id)
    }

    fn index_metadata(&self, id: u32, metadata: &VectorMetadata) {
        {
            let mut entry = self.by_source.entry(metadata.source.clone()).or_default();
            if entry.ids.insert(id) && metadata.content_type != ENTITY_DESCRIPTION {
                entry.update_count += 1;
            }
        }
        self.by_content_id
            .entry(metadata.id.clone())
            .or_default()
            .push(id);
        self.text_index.add(id, &metadata.text);
    }

    fn unindex_metadata(&self, id: u32, metadata: &VectorMetadata) {
        self.by_source.remove_if_mut(&metadata.source, |_, entry| {
            if entry.ids.remove(&id) && metadata.content_type != ENTITY_DESCRIPTION {
                entry.update_count = entry.update_count.saturating_sub(1);
            }
            entry.ids.is_empty()
        });
        self.by_content_id.remove_if_mut(&metadata.id, |_, ids| {
            ids.retain(|&v| v != id);
            ids.is_empty()
        });
        self.text_index.remove(id, &metadata.text);
    }

    /// Bring freshly stored vectors into the HNSW graph if it is live
    /// (auto-built or via [`Self::build_index`]), or build it once if HNSW
    /// is enabled and the DB just crossed the exact-search threshold.
    fn index_new_vectors(&self, ids: &[u32]) {
        if !self.hnsw_active.load(Ordering::Acquire) {
            if !self.config.enable_hnsw_index
                || self.vectors.len() <= self.config.exact_search_threshold
            {
                return;
            }
            let _guard = self.build_lock.lock();
            if !self.hnsw_active.load(Ordering::Acquire) {
                info!(
                    "Vector DB crossed {} vectors, building HNSW index",
                    self.config.exact_search_threshold
                );
                self.build_graph_locked();
                return;
            }
        }
        // The graph may have been built concurrently from a snapshot that
        // already holds some of these.
        for &id in ids {
            if !self.hnsw_index.contains(id) {
                self.insert_into_graph(id);
            }
        }
    }

    /// Search for similar vectors
    pub fn search(&self, query_vector: &[f32], k: usize) -> Result<Vec<SearchMatch>> {
        // Use exact (linear) scan for small-to-medium DBs — HNSW post-filter
        // misses sparse clusters and is only worth it for very large DBs.
        let mode = if self.vectors.len() <= self.config.exact_search_threshold {
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
        self.check_query(query_vector)?;
        let start_time = std::time::Instant::now();
        let query_norm = l2_norm(query_vector);

        let results = self.ranked(query_vector, query_norm, k, mode, ef_search_override);
        let matches = self.to_matches(results, k, None);

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

    fn check_query(&self, query_vector: &[f32]) -> Result<()> {
        if query_vector.len() != self.config.dimension {
            return Err(anyhow::anyhow!(
                "Query vector dimension {} does not match expected {}",
                query_vector.len(),
                self.config.dimension
            ));
        }
        Ok(())
    }

    /// Top-`k` ids for an unfiltered query under `mode`.
    fn ranked(
        &self,
        query_vector: &[f32],
        query_norm: f32,
        k: usize,
        mode: SearchMode,
        ef_search_override: Option<usize>,
    ) -> Vec<SearchResult> {
        match mode {
            SearchMode::Exact => {
                debug!("Using exact search (linear scan)");
                self.scan_all(query_vector, query_norm, k, None)
            }
            SearchMode::Approximate | SearchMode::Balanced => {
                let ef_search = ef_search_override.unwrap_or_else(|| match mode {
                    SearchMode::Approximate => SearchQualityPreset::Fast.ef_search(),
                    SearchMode::Balanced => SearchQualityPreset::Balanced.ef_search(),
                    SearchMode::Exact => self.config.ef_search,
                });
                match self.graph_search(query_vector, query_norm, k, ef_search) {
                    Some(results) => {
                        debug!("Using HNSW search with ef_search={}", ef_search);
                        results
                    }
                    None => {
                        debug!("HNSW index not available, falling back to linear search");
                        self.scan_all(query_vector, query_norm, k, None)
                    }
                }
            }
        }
    }

    /// Exact scan over every vector, filtering on metadata before scoring.
    fn scan_all(
        &self,
        query_vector: &[f32],
        query_norm: f32,
        k: usize,
        filter: Option<&dyn Fn(&VectorMetadata) -> bool>,
    ) -> Vec<SearchResult> {
        let mut top = TopK::new(k);
        for entry in self.vectors.iter() {
            let id = *entry.key();
            if let Some(filter) = filter {
                match self.metadata.get(&id) {
                    Some(m) if filter(&m) => {}
                    _ => continue,
                }
            }
            let stored = entry.value();
            top.push(
                id,
                cosine_with_norms(query_vector, &stored.vector, query_norm, stored.magnitude),
            );
        }
        top.into_sorted()
    }

    /// Exact scan over the given ids only.
    fn scan_ids(
        &self,
        ids: &[u32],
        query_vector: &[f32],
        query_norm: f32,
        k: usize,
        filter: Option<&dyn Fn(&VectorMetadata) -> bool>,
    ) -> Vec<SearchResult> {
        let mut top = TopK::new(k);
        for &id in ids {
            if let Some(filter) = filter {
                match self.metadata.get(&id) {
                    Some(m) if filter(&m) => {}
                    _ => continue,
                }
            }
            if let Some(stored) = self.vectors.get(&id) {
                top.push(
                    id,
                    cosine_with_norms(query_vector, &stored.vector, query_norm, stored.magnitude),
                );
            }
        }
        top.into_sorted()
    }

    /// Attach metadata to the ranked ids, applying `filter` and stopping at
    /// `k`. Metadata is cloned only for the returned matches.
    fn to_matches(
        &self,
        results: Vec<SearchResult>,
        k: usize,
        filter: Option<&dyn Fn(&VectorMetadata) -> bool>,
    ) -> Vec<SearchMatch> {
        let mut matches = Vec::with_capacity(results.len().min(k));
        for result in results {
            if matches.len() >= k {
                break;
            }
            if let Some(metadata) = self.metadata.get(&result.id) {
                if filter.is_some_and(|f| !f(&metadata)) {
                    continue;
                }
                matches.push(SearchMatch {
                    vector_id: result.id,
                    similarity: result.similarity,
                    metadata: metadata.clone(),
                });
            }
        }
        matches
    }

    /// HNSW search with configurable ef_search parameter.
    ///
    /// Returns `None` when the graph is not live or its entry point is gone,
    /// so the caller can fall back to an exact scan.
    fn graph_search(
        &self,
        query_vector: &[f32],
        query_norm: f32,
        k: usize,
        ef_search: usize,
    ) -> Option<Vec<SearchResult>> {
        if !self.hnsw_active.load(Ordering::Acquire) {
            return None;
        }
        let mut results = self.graph_candidates(query_vector, query_norm, ef_search.max(k))?;
        results.truncate(k);
        Some(results)
    }

    /// Greedy best-first walk over the graph keeping the `ef` best nodes,
    /// sorted best first. Nodes whose vector has been removed are skipped.
    fn graph_candidates(
        &self,
        query_vector: &[f32],
        query_norm: f32,
        ef: usize,
    ) -> Option<Vec<SearchResult>> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        let entry_point = self.hnsw_index.get_entry_point()?;
        let initial_similarity = self.similarity_to(query_vector, query_norm, entry_point)?;

        // Max-heap for candidates (best similarity first via Ord impl)
        let mut candidates: BinaryHeap<SearchResult> = BinaryHeap::new();
        // Min-heap for results (worst similarity first, to efficiently drop worst)
        let mut results: BinaryHeap<Reverse<SearchResult>> = BinaryHeap::new();
        let mut visited = HashSet::new();
        let mut neighbours = Vec::new();

        let start = SearchResult {
            id: entry_point,
            similarity: initial_similarity,
        };
        candidates.push(start.clone());
        results.push(Reverse(start));
        visited.insert(entry_point);

        while let Some(current) = candidates.pop() {
            // Early termination: if current candidate is worse than worst result, stop
            if results.len() >= ef
                && let Some(Reverse(worst)) = results.peek()
                && current.similarity < worst.similarity
            {
                break;
            }

            if !self
                .hnsw_index
                .connections_into(current.id, &mut neighbours)
            {
                continue;
            }
            for &connected_id in &neighbours {
                if !visited.insert(connected_id) {
                    continue;
                }
                let Some(similarity) = self.similarity_to(query_vector, query_norm, connected_id)
                else {
                    continue;
                };
                let result = SearchResult {
                    id: connected_id,
                    similarity,
                };
                candidates.push(result.clone());
                results.push(Reverse(result));
                if results.len() > ef {
                    results.pop(); // Removes worst (smallest similarity)
                }
            }
        }

        Some(results.into_sorted_vec().into_iter().map(|r| r.0).collect())
    }

    /// Cosine similarity of `query_vector` (whose L2 norm is `query_norm`) to
    /// the stored vector `vector_id`.
    pub fn similarity_to(
        &self,
        query_vector: &[f32],
        query_norm: f32,
        vector_id: u32,
    ) -> Option<f32> {
        self.vectors.get(&vector_id).map(|stored| {
            cosine_with_norms(query_vector, &stored.vector, query_norm, stored.magnitude)
        })
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

    /// Assign a random layer to a new vector using exponential distribution.
    ///
    /// The graph itself is single-layer; the layer only decides which node
    /// becomes the search entry point (the highest one).
    fn random_layer(&self) -> usize {
        // ml = 1 / ln(M) where M is max_connections
        let ml = 1.0 / (self.config.max_connections.max(2) as f64).ln();
        let r: f64 = rand::random::<f64>().max(1e-10); // Avoid log(0)
        let layer = (-r.ln() * ml).floor() as usize;
        // Cap at num_layers to prevent unbounded growth
        layer.min(self.config.num_layers.saturating_sub(1))
    }

    /// Exact k nearest *indexed* nodes — used while the graph is still too
    /// small for a graph walk to be cheaper than scoring every node.
    fn nearest_graph_nodes(
        &self,
        query_vector: &[f32],
        query_norm: f32,
        k: usize,
    ) -> Vec<SearchResult> {
        let ids = self.hnsw_index.node_ids();
        self.scan_ids(&ids, query_vector, query_norm, k, None)
    }

    /// Add `to_id` to `from_id`'s adjacency list
    fn add_connection(&self, from_id: u32, to_id: u32) {
        self.hnsw_index
            .connections
            .entry(from_id)
            .or_default()
            .push(to_id);
    }

    /// Prune connections to keep only the best max_connections neighbors
    fn prune_connections(&self, vector_id: u32, max_connections: usize) {
        let Some((query, query_norm)) = self
            .vectors
            .get(&vector_id)
            .map(|v| (v.vector.clone(), v.magnitude))
        else {
            return;
        };

        if let Some(mut entry) = self.hnsw_index.connections.get_mut(&vector_id) {
            let connections = entry.value_mut();
            if connections.len() <= max_connections {
                return;
            }
            let mut top = TopK::new(max_connections);
            for &neighbor_id in connections.iter() {
                if let Some(sim) = self.similarity_to(&query, query_norm, neighbor_id) {
                    top.push(neighbor_id, sim);
                }
            }
            *connections = top.into_sorted().into_iter().map(|r| r.id).collect();
        }
    }

    /// Insert one stored vector into the live graph.
    ///
    /// Neighbours come from a graph walk with `ef_construction` (exact scan
    /// while the graph is small), so an insert costs O(ef · M), not O(n).
    fn insert_into_graph(&self, vector_id: u32) {
        let max_connections = self.config.max_connections;
        let Some((query_vector, query_norm)) = self
            .vectors
            .get(&vector_id)
            .map(|v| (v.vector.clone(), v.magnitude))
        else {
            return; // removed concurrently
        };

        let layer = self.random_layer();
        let ef_construction = self.config.ef_construction.max(max_connections * 2);

        let neighbors = if self.hnsw_index.len() <= ef_construction {
            self.nearest_graph_nodes(&query_vector, query_norm, max_connections)
        } else {
            self.graph_candidates(&query_vector, query_norm, ef_construction)
                .unwrap_or_else(|| {
                    self.nearest_graph_nodes(&query_vector, query_norm, max_connections)
                })
        };

        let selected: Vec<u32> = neighbors
            .iter()
            .filter(|r| r.id != vector_id)
            .take(max_connections)
            .map(|r| r.id)
            .collect();

        // Register the node with its outgoing edges first, then add the
        // reverse edges, so nothing overwrites edges added to it meanwhile.
        self.hnsw_index
            .add_vector(vector_id, layer, selected.clone());
        for &neighbor_id in &selected {
            self.add_connection(neighbor_id, vector_id);
            self.prune_connections(neighbor_id, max_connections);
        }

        debug!(
            "Indexed vector {} at layer {} with {} neighbors",
            vector_id,
            layer,
            selected.len()
        );
    }

    /// Rebuild the graph from all stored vectors. Caller holds `build_lock`.
    fn build_graph_locked(&self) {
        self.hnsw_index.clear();
        // Live before the snapshot: inserts racing with the build add
        // themselves incrementally instead of being missed.
        self.hnsw_active.store(true, Ordering::Release);

        let mut vector_ids: Vec<u32> = self.vectors.iter().map(|entry| *entry.key()).collect();
        vector_ids.sort_unstable();

        // Seed sequentially until graph walks take over from exact neighbour
        // scans, then insert the rest in parallel (inserts are thread-safe).
        let seed = vector_ids.len().min(
            self.config
                .ef_construction
                .max(self.config.max_connections * 2)
                + 1,
        );
        let insert = |&vector_id: &u32| {
            if !self.hnsw_index.contains(vector_id) {
                self.insert_into_graph(vector_id);
            }
        };
        vector_ids[..seed].iter().for_each(insert);
        vector_ids[seed..].par_iter().for_each(insert);
        self.stats.is_built.store(true, Ordering::Relaxed);
    }

    /// Get metadata for a vector
    pub fn get_metadata(&self, vector_id: u32) -> Option<VectorMetadata> {
        self.metadata.get(&vector_id).map(|entry| entry.clone())
    }

    /// Remove a vector from the database
    pub fn remove_vector(&self, vector_id: u32) -> Result<bool> {
        let removed_vector = self.vectors.remove(&vector_id);
        if let Some((_, metadata)) = self.metadata.remove(&vector_id) {
            self.unindex_metadata(vector_id, &metadata);
        }

        if let Some((_, removed)) = removed_vector {
            // Update statistics - match size calculation from add_vector
            self.stats
                .record_vector_removed(stored_vector_size(&removed));
            self.hnsw_index.remove_vector(vector_id);
            debug!("Removed vector {} from database", vector_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Remove several vectors with a single pass over the HNSW graph.
    /// Returns how many existed.
    pub fn remove_vectors(&self, vector_ids: &[u32]) -> usize {
        let mut removed_ids = HashSet::with_capacity(vector_ids.len());
        for &vector_id in vector_ids {
            if let Some((_, metadata)) = self.metadata.remove(&vector_id) {
                self.unindex_metadata(vector_id, &metadata);
            }
            if let Some((_, removed)) = self.vectors.remove(&vector_id) {
                self.stats
                    .record_vector_removed(stored_vector_size(&removed));
                removed_ids.insert(vector_id);
            }
        }
        self.hnsw_index.remove_vectors(&removed_ids);
        removed_ids.len()
    }

    /// Remove every vector whose `metadata.id` is `content_id`. Returns how
    /// many were removed.
    pub fn remove_by_content_id(&self, content_id: &str) -> usize {
        let ids = self.find_vector_ids_by_content_id(content_id);
        self.remove_vectors(&ids)
    }

    /// Remove every vector whose `metadata.source` is `source` (e.g. all of
    /// a session's vectors). Returns how many were removed.
    pub fn remove_source(&self, source: &str) -> usize {
        let ids = self.source_vector_ids(source);
        self.remove_vectors(&ids)
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

    /// Check if database has embeddings for a session. O(1).
    pub fn has_session_embeddings(&self, session_id: &str) -> bool {
        self.by_source
            .get(session_id)
            .is_some_and(|entry| !entry.ids.is_empty())
    }

    /// Count embeddings for a session. O(1).
    pub fn count_session_embeddings(&self, session_id: &str) -> usize {
        self.by_source
            .get(session_id)
            .map_or(0, |entry| entry.ids.len())
    }

    /// Check if session has update embeddings (not just entities). O(1).
    pub fn has_session_update_embeddings(&self, session_id: &str) -> bool {
        self.by_source
            .get(session_id)
            .is_some_and(|entry| entry.update_count > 0)
    }

    /// Check if a specific update_id exists as an embedding. O(1).
    pub fn has_update_embedding(&self, update_id: &str) -> bool {
        self.by_content_id.contains_key(update_id)
    }

    /// Find vector_id by content_id (for removal operations). O(1).
    ///
    /// The words of `query` worth searching for: deduplicated, without the
    /// ones too common in the stored text to discriminate (see
    /// `text_index`). For every lexical search, whatever the backend.
    pub fn query_terms(&self, query: &str) -> Vec<String> {
        self.text_index.query_terms(query)
    }

    /// BM25 top-`k` for `terms` (from [`Self::query_terms`]) over the
    /// vectors' text, restricted to `sources` (empty = all), best first;
    /// `similarity` is the cosine to `query_vector`. `None` when BM25
    /// postings are off (`VectorDbConfig::full_text_index`).
    pub fn search_text(
        &self,
        terms: &[String],
        query_vector: &[f32],
        k: usize,
        sources: &[String],
    ) -> Option<Vec<SearchMatch>> {
        if !self.text_index.has_postings() {
            return None;
        }
        let keep = |id: u32| {
            sources.is_empty()
                || self
                    .metadata
                    .get(&id)
                    .is_some_and(|m| sources.iter().any(|s| *s == m.source))
        };
        let query_norm = l2_norm(query_vector);
        Some(
            self.text_index
                .search(terms, k, keep)
                .into_iter()
                .filter_map(|(id, _)| {
                    let metadata = self.metadata.get(&id)?.clone();
                    Some(SearchMatch {
                        vector_id: id,
                        similarity: self
                            .similarity_to(query_vector, query_norm, id)
                            .unwrap_or(0.0),
                        metadata,
                    })
                })
                .collect(),
        )
    }

    /// If several vectors share the content id, returns the oldest; use
    /// [`Self::find_vector_ids_by_content_id`] to get all of them.
    pub fn find_vector_id_by_content_id(&self, content_id: &str) -> Option<u32> {
        self.by_content_id
            .get(content_id)
            .and_then(|ids| ids.first().copied())
    }

    /// All vector ids whose `metadata.id` is `content_id`, oldest first.
    pub fn find_vector_ids_by_content_id(&self, content_id: &str) -> Vec<u32> {
        self.by_content_id
            .get(content_id)
            .map(|ids| ids.clone())
            .unwrap_or_default()
    }

    /// All vector ids whose `metadata.source` is `source`, in no particular
    /// order. O(vectors in that source).
    pub fn source_vector_ids(&self, source: &str) -> Vec<u32> {
        self.by_source
            .get(source)
            .map(|entry| entry.ids.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Get list of update IDs that exist in vector DB for a session.
    /// O(vectors in that session).
    pub fn get_vectorized_update_ids(&self, session_id: &str) -> Vec<String> {
        self.source_vector_ids(session_id)
            .into_iter()
            .filter_map(|id| {
                self.metadata
                    .get(&id)
                    .and_then(|m| (m.content_type != ENTITY_DESCRIPTION).then(|| m.id.clone()))
            })
            .collect()
    }

    /// Build HNSW index for all vectors, whatever the DB size. From then on
    /// inserts keep the graph up to date.
    pub fn build_index(&self) -> Result<()> {
        info!("Building HNSW index for {} vectors", self.vectors.len());
        let _guard = self.build_lock.lock();
        self.build_graph_locked();
        info!("HNSW index built successfully");
        Ok(())
    }

    /// Clear all vectors from the database
    pub fn clear(&self) -> Result<()> {
        let _guard = self.build_lock.lock();
        let vector_count = self.vectors.len();

        self.vectors.clear();
        self.metadata.clear();
        self.by_source.clear();
        self.by_content_id.clear();
        self.text_index.clear();
        self.hnsw_index.clear();
        self.hnsw_active.store(false, Ordering::Release);

        // Reset statistics
        self.stats.total_vectors.store(0, Ordering::Relaxed);
        self.stats.memory_usage_bytes.store(0, Ordering::Relaxed);
        self.stats.is_built.store(false, Ordering::Relaxed);

        // Reset ID counter
        self.next_id.store(0, Ordering::Relaxed);

        info!("Cleared {} vectors from database", vector_count);
        Ok(())
    }

    /// Search with custom filter
    ///
    /// The filter runs during the scan (before scoring) and metadata is
    /// cloned only for the returned matches. For source filters prefer
    /// [`Self::search_in_sources_with_filter`], which scores only that
    /// source's vectors.
    pub fn search_with_filter<F>(
        &self,
        query_vector: &[f32],
        k: usize,
        filter: F,
    ) -> Result<Vec<SearchMatch>>
    where
        F: Fn(&VectorMetadata) -> bool,
    {
        self.check_query(query_vector)?;
        let start_time = std::time::Instant::now();
        let query_norm = l2_norm(query_vector);
        let total_vectors = self.vectors.len();

        // Small DB: exact filtered scan, guaranteed to find sparse matches.
        // Large DB: HNSW with a widened candidate list, filtered afterwards.
        let graph = if total_vectors > self.config.exact_search_threshold {
            let search_k = k.saturating_mul(10).min(total_vectors);
            let ef = self.config.ef_search.max(search_k.saturating_mul(2));
            debug!(
                "search_with_filter: k={}, search_k={}, dynamic_ef_search={}",
                k, search_k, ef
            );
            self.graph_search(query_vector, query_norm, search_k, ef)
        } else {
            None
        };

        let matches = match graph {
            Some(candidates) => self.to_matches(candidates, k, Some(&filter)),
            None => {
                debug!(
                    "search_with_filter: exact filtered scan over {} vectors",
                    total_vectors
                );
                let ranked = self.scan_all(query_vector, query_norm, k, Some(&filter));
                self.to_matches(ranked, k, None)
            }
        };

        self.stats
            .record_search(start_time.elapsed().as_micros() as u64);
        log_content_types(&matches);
        Ok(matches)
    }

    /// Search in specific source
    pub fn search_in_source(
        &self,
        query_vector: &[f32],
        k: usize,
        source: &str,
    ) -> Result<Vec<SearchMatch>> {
        self.search_in_sources(query_vector, k, &[source])
    }

    /// Exact search over the vectors of the given sources only (e.g. a set
    /// of session ids). Cost is O(vectors in those sources), independent of
    /// the DB size, and never misses matches the way a post-filtered HNSW
    /// search can.
    pub fn search_in_sources<S: AsRef<str>>(
        &self,
        query_vector: &[f32],
        k: usize,
        sources: &[S],
    ) -> Result<Vec<SearchMatch>> {
        self.search_sources_inner(query_vector, k, sources, None)
    }

    /// [`Self::search_in_sources`] plus an extra metadata predicate (e.g. a
    /// date range), applied before scoring.
    pub fn search_in_sources_with_filter<S, F>(
        &self,
        query_vector: &[f32],
        k: usize,
        sources: &[S],
        filter: F,
    ) -> Result<Vec<SearchMatch>>
    where
        S: AsRef<str>,
        F: Fn(&VectorMetadata) -> bool,
    {
        self.search_sources_inner(query_vector, k, sources, Some(&filter))
    }

    fn search_sources_inner<S: AsRef<str>>(
        &self,
        query_vector: &[f32],
        k: usize,
        sources: &[S],
        filter: Option<&dyn Fn(&VectorMetadata) -> bool>,
    ) -> Result<Vec<SearchMatch>> {
        self.check_query(query_vector)?;
        let start_time = std::time::Instant::now();
        let query_norm = l2_norm(query_vector);

        let mut seen: HashSet<&str> = HashSet::with_capacity(sources.len());
        let mut ids = Vec::new();
        for source in sources {
            let source = source.as_ref();
            if seen.insert(source)
                && let Some(entry) = self.by_source.get(source)
            {
                ids.extend(entry.ids.iter().copied());
            }
        }

        let ranked = self.scan_ids(&ids, query_vector, query_norm, k, filter);
        let matches = self.to_matches(ranked, k, None);

        self.stats
            .record_search(start_time.elapsed().as_micros() as u64);
        debug!(
            "search_in_sources: scored {} vectors from {} sources, {} matches",
            ids.len(),
            seen.len(),
            matches.len()
        );
        Ok(matches)
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

/// Log the content-type mix of a result set (debugging cross-language search).
fn log_content_types(matches: &[SearchMatch]) {
    if log::log_enabled!(log::Level::Debug) {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for m in matches {
            *counts.entry(m.metadata.content_type.as_str()).or_insert(0) += 1;
        }
        debug!("search_with_filter: ContentType distribution: {:?}", counts);
    }
}

/// Approximate bytes occupied by a stored vector (used for memory stats).
fn stored_vector_size(stored: &StoredVector) -> usize {
    std::mem::size_of::<StoredVector>()
        + stored.vector.len() * std::mem::size_of::<f32>()
        + stored.quantized.as_ref().map(|q| q.len()).unwrap_or(0)
        + stored.pq_codes.as_ref().map(|pq| pq.len()).unwrap_or(0)
}
