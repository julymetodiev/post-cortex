// Copyright (c) 2025 Julius ML
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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//! Query caching system for semantic search results
//!
//! This is a cleaned version of the query cache with removed unused code.
//! All functionality is preserved, but unused components have been removed.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tracing::{debug, info};
use uuid::Uuid;

use crate::core::content_vectorizer::SemanticSearchResult;

/// Configuration for query cache
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryCacheConfig {
    /// Maximum number of cached queries
    pub max_cache_size: usize,
    /// Time-to-live for cached results in minutes
    pub ttl_minutes: i64,
    /// Similarity threshold for query matching (0.0-1.0)
    pub similarity_threshold: f32,
    /// Enable intelligent prefetching
    pub enable_prefetching: bool,
    /// Maximum number of query variations to prefetch
    pub max_prefetch_variations: usize,
    /// Enable cache statistics collection
    pub enable_stats: bool,
}

impl Default for QueryCacheConfig {
    fn default() -> Self {
        Self {
            max_cache_size: 1000,
            ttl_minutes: 30,
            similarity_threshold: 0.85,
            enable_prefetching: true,
            max_prefetch_variations: 5,
            enable_stats: true,
        }
    }
}

/// A cached query with atomic access tracking
#[derive(Debug, Serialize, Deserialize)]
pub struct CachedQuery {
    /// Unique identifier for the query
    pub id: Uuid,
    /// Original query text
    pub query_text: String,
    /// Query embedding vector
    pub query_vector: Vec<f32>,
    /// Cached search results
    pub results: Vec<SemanticSearchResult>,
    /// Timestamp when cached
    pub cached_at: DateTime<Utc>,
    /// Last access timestamp (atomic)
    last_accessed: AtomicU64,
    /// Number of times this query was accessed (atomic)
    access_count: AtomicU64,
    /// Query parameters hash for exact matching
    pub params_hash: u64,
    /// Session ID that created this query (optional)
    pub session_id: Option<Uuid>,
    /// Cache hit efficiency score stored as u32 bits for atomic operations
    efficiency_score_bits: AtomicU64,
}

impl CachedQuery {
    /// Create a new cached query
    pub fn new(
        query_text: String,
        query_vector: Vec<f32>,
        results: Vec<SemanticSearchResult>,
        params_hash: u64,
        session_id: Option<Uuid>,
    ) -> Self {
        let now = Utc::now();
        let now_timestamp = now.timestamp() as u64;

        Self {
            id: Uuid::new_v4(),
            query_text,
            query_vector,
            results,
            cached_at: now,
            last_accessed: AtomicU64::new(now_timestamp),
            access_count: AtomicU64::new(0),
            params_hash,
            session_id,
            efficiency_score_bits: AtomicU64::new(1.0f32.to_bits() as u64),
        }
    }

    /// Check if the cached query has expired
    pub fn is_expired(&self, ttl_minutes: i64) -> bool {
        let ttl_duration = Duration::minutes(ttl_minutes);
        Utc::now() - self.cached_at > ttl_duration
    }

    /// Update access statistics (via CAS)
    pub fn mark_accessed(&self) {
        let now = Utc::now();
        let now_timestamp = now.timestamp() as u64;
        self.last_accessed.store(now_timestamp, Ordering::Relaxed);
        self.access_count.fetch_add(1, Ordering::Relaxed);

        // Update efficiency score using CAS loop for atomicity
        // Reuse `now` to avoid double syscall
        let hours_since_cached = (now - self.cached_at).num_hours().max(1) as f32;
        let recency_factor = 1.0 / (1.0 + hours_since_cached / 24.0); // Decay over days

        loop {
            let old_bits = self.efficiency_score_bits.load(Ordering::Relaxed);
            let count = self.access_count.load(Ordering::Relaxed);
            let frequency_factor = (count as f32).ln().max(1.0);
            let score = recency_factor * frequency_factor;

            if self
                .efficiency_score_bits
                .compare_exchange_weak(
                    old_bits,
                    score.to_bits() as u64,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
    }

    /// Get efficiency score
    pub fn efficiency_score(&self) -> f32 {
        f32::from_bits(self.efficiency_score_bits.load(Ordering::Relaxed) as u32)
    }

    /// Calculate similarity with another query vector
    pub fn similarity_with(&self, other_vector: &[f32]) -> f32 {
        if self.query_vector.len() != other_vector.len() {
            return 0.0;
        }

        let dot_product: f32 = self
            .query_vector
            .iter()
            .zip(other_vector.iter())
            .map(|(a, b)| a * b)
            .sum();

        let norm_a: f32 = self.query_vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = other_vector.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot_product / (norm_a * norm_b)
        }
    }
}

/// Cache statistics using atomic operations
#[derive(Debug, Serialize, Deserialize)]
pub struct QueryCacheStats {
    /// Total number of queries processed (atomic)
    pub total_queries: AtomicU64,
    /// Number of cache hits (atomic)
    pub cache_hits: AtomicU64,
    /// Number of cache misses (atomic)
    pub cache_misses: AtomicU64,
    /// Number of expired entries removed (atomic)
    pub expired_removed: AtomicU64,
    /// Number of entries evicted due to size limit (atomic)
    pub evicted_entries: AtomicU64,
    /// Average similarity score for cache hits stored as u32 bits
    avg_hit_similarity_bits: AtomicU64,
    /// Current cache size (atomic)
    pub current_cache_size: AtomicUsize,
    /// Memory usage estimation in bytes (atomic)
    pub estimated_memory_bytes: AtomicUsize,
    /// Hit rate percentage (computed)
    pub hit_rate: f32,
    /// Average query processing time saved (ms)
    pub avg_time_saved_ms: f32,
}

impl Default for QueryCacheStats {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryCacheStats {
    /// Create new empty statistics
    pub fn new() -> Self {
        Self {
            total_queries: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            expired_removed: AtomicU64::new(0),
            evicted_entries: AtomicU64::new(0),
            avg_hit_similarity_bits: AtomicU64::new(0.0f32.to_bits() as u64),
            current_cache_size: AtomicUsize::new(0),
            estimated_memory_bytes: AtomicUsize::new(0),
            hit_rate: 0.0,
            avg_time_saved_ms: 0.0,
        }
    }

    /// Record a cache hit (via CAS)
    pub fn record_hit(&self, similarity: f32) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);

        // Update average hit similarity using CAS loop for atomicity
        loop {
            let old_bits = self.avg_hit_similarity_bits.load(Ordering::Relaxed);
            let hits = self.cache_hits.load(Ordering::Relaxed);
            let current_avg = f32::from_bits(old_bits as u32);
            let new_avg = if hits == 1 {
                similarity
            } else {
                ((current_avg * (hits as f32 - 1.0)) + similarity) / hits as f32
            };

            if self
                .avg_hit_similarity_bits
                .compare_exchange_weak(
                    old_bits,
                    new_avg.to_bits() as u64,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
    }

    /// Record a cache miss
    pub fn record_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Get average hit similarity
    pub fn avg_hit_similarity(&self) -> f32 {
        f32::from_bits(self.avg_hit_similarity_bits.load(Ordering::Relaxed) as u32)
    }

    /// Get snapshot of current stats
    pub fn snapshot(&self) -> QueryCacheStatsSnapshot {
        let total = self.total_queries.load(Ordering::Relaxed);
        let hits = self.cache_hits.load(Ordering::Relaxed);

        let hit_rate = if total > 0 {
            (hits as f32 / total as f32) * 100.0
        } else {
            0.0
        };

        QueryCacheStatsSnapshot {
            total_queries: total,
            cache_hits: hits,
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            expired_removed: self.expired_removed.load(Ordering::Relaxed),
            evicted_entries: self.evicted_entries.load(Ordering::Relaxed),
            avg_hit_similarity: self.avg_hit_similarity(),
            current_cache_size: self.current_cache_size.load(Ordering::Relaxed),
            estimated_memory_bytes: self.estimated_memory_bytes.load(Ordering::Relaxed),
            hit_rate,
            avg_time_saved_ms: 150.0, // Approximate time to generate embeddings
        }
    }
}

/// Snapshot of cache statistics for reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCacheStatsSnapshot {
    pub total_queries: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub expired_removed: u64,
    pub evicted_entries: u64,
    pub avg_hit_similarity: f32,
    pub current_cache_size: usize,
    pub estimated_memory_bytes: usize,
    pub hit_rate: f32,
    pub avg_time_saved_ms: f32,
}

/// Query pattern for prefetching (simplified)
#[derive(Debug)]
struct QueryPattern {
    frequency: AtomicU64,
    last_seen: AtomicU64,
}

/// Query cache using DashMap and atomic operations
pub struct QueryCache {
    /// Cache storage - DashMap
    cache: Arc<DashMap<Uuid, CachedQuery>>,

    /// Cache configuration - immutable after creation
    config: QueryCacheConfig,

    /// Performance statistics using atomics
    stats: Arc<QueryCacheStats>,

    /// Query patterns for prefetching - DashMap
    patterns: Arc<DashMap<String, QueryPattern>>,

    /// Recent queries for pattern detection - DashMap
    recent_queries: Arc<DashMap<String, AtomicU64>>,
}

impl QueryCache {
    /// Create a new query cache
    pub fn new(config: QueryCacheConfig) -> Self {
        info!(
            "Initializing query cache with max size: {}",
            config.max_cache_size
        );

        Self {
            cache: Arc::new(DashMap::new()),
            config,
            stats: Arc::new(QueryCacheStats::new()),
            patterns: Arc::new(DashMap::new()),
            recent_queries: Arc::new(DashMap::new()),
        }
    }

    /// Search for cached results for a query
    pub fn search(
        &self,
        query_text: &str,
        query_vector: &[f32],
        params_hash: u64,
    ) -> Option<Vec<SemanticSearchResult>> {
        // Update total queries count
        self.stats.total_queries.fetch_add(1, Ordering::Relaxed);

        // First, try exact parameter match
        if let Some(results) = self.find_exact_match(params_hash) {
            self.stats.record_hit(1.0);
            return Some(results);
        }

        // Then try similarity-based matching
        // Now checks params_hash to prevent cross-session result leakage
        if let Some((results, similarity)) = self.find_similar_query(query_vector, params_hash) {
            self.stats.record_hit(similarity);
            return Some(results);
        }

        // Record cache miss
        self.stats.record_miss();

        // Update query patterns for future prefetching
        if self.config.enable_prefetching {
            self.update_query_patterns(query_text);
        }

        None
    }

    /// Cache query results
    pub fn cache_results(
        &self,
        query_text: String,
        query_vector: Vec<f32>,
        results: Vec<SemanticSearchResult>,
        params_hash: u64,
        session_id: Option<Uuid>,
    ) -> Result<()> {
        // Clean up expired entries first
        self.cleanup_expired()?;

        let cached_query = CachedQuery::new(
            query_text.clone(),
            query_vector,
            results,
            params_hash,
            session_id,
        );

        let query_id = cached_query.id;

        // Check if we need to evict before inserting
        let current_size = self.cache.len();
        if current_size >= self.config.max_cache_size {
            self.evict_least_efficient();
        }

        // Insert new entry
        self.cache.insert(query_id, cached_query);

        // Update statistics
        self.stats
            .current_cache_size
            .store(self.cache.len(), Ordering::Relaxed);
        self.stats
            .estimated_memory_bytes
            .store(self.estimate_memory_usage(), Ordering::Relaxed);

        debug!("Cached query results for: {}", query_text);
        Ok(())
    }

    /// Find exact parameter match
    fn find_exact_match(&self, params_hash: u64) -> Option<Vec<SemanticSearchResult>> {
        for entry in self.cache.iter() {
            let cached_query = entry.value();
            if cached_query.params_hash == params_hash
                && !cached_query.is_expired(self.config.ttl_minutes)
            {
                // Update access statistics
                cached_query.mark_accessed();
                return Some(cached_query.results.clone());
            }
        }
        None
    }

    /// Find similar query based on vector similarity
    ///
    /// CRITICAL: This method now checks params_hash to ensure cached results
    /// match the current query parameters (session filter, date range, etc.)
    fn find_similar_query(
        &self,
        query_vector: &[f32],
        params_hash: u64,
    ) -> Option<(Vec<SemanticSearchResult>, f32)> {
        let mut best_match: Option<(Vec<SemanticSearchResult>, f32, Uuid)> = None;
        let mut best_similarity = 0.0f32;

        for entry in self.cache.iter() {
            let cached_query = entry.value();

            if cached_query.is_expired(self.config.ttl_minutes) {
                continue;
            }

            // CRITICAL FIX: Check params_hash matches before considering similarity
            // This prevents returning cached results from different sessions/filters
            // Bug: Previously, similar queries with different session filters
            // would return wrong cached results from other sessions
            if cached_query.params_hash != params_hash {
                continue;
            }

            let similarity = cached_query.similarity_with(query_vector);

            if similarity >= self.config.similarity_threshold && similarity > best_similarity {
                best_similarity = similarity;
                best_match = Some((cached_query.results.clone(), similarity, cached_query.id));
            }
        }

        if let Some((results, similarity, query_id)) = best_match {
            // Update access statistics for the matched query
            if let Some(entry) = self.cache.get(&query_id) {
                entry.mark_accessed();
            }
            Some((results, similarity))
        } else {
            None
        }
    }

    /// Clean up expired cache entries
    fn cleanup_expired(&self) -> Result<()> {
        let mut removed_count = 0;

        self.cache.retain(|_, cached_query| {
            let expired = cached_query.is_expired(self.config.ttl_minutes);
            if expired {
                removed_count += 1;
            }
            !expired
        });

        if removed_count > 0 {
            self.stats
                .expired_removed
                .fetch_add(removed_count as u64, Ordering::Relaxed);
            self.stats
                .current_cache_size
                .store(self.cache.len(), Ordering::Relaxed);
            self.stats
                .estimated_memory_bytes
                .store(self.estimate_memory_usage(), Ordering::Relaxed);

            debug!("Removed {} expired cache entries", removed_count);
        }

        Ok(())
    }

    /// Evict least efficient cache entry
    fn evict_least_efficient(&self) {
        if self.cache.is_empty() {
            return;
        }

        // Find the least efficient entry
        let mut worst_id: Option<Uuid> = None;
        let mut worst_score = f32::INFINITY;

        for entry in self.cache.iter() {
            let score = entry.value().efficiency_score();
            if score < worst_score {
                worst_score = score;
                worst_id = Some(*entry.key());
            }
        }

        if let Some(id) = worst_id {
            // Only increment counter if we actually removed the entry
            // (another thread may have already removed it)
            if self.cache.remove(&id).is_some() {
                self.stats.evicted_entries.fetch_add(1, Ordering::Relaxed);

                debug!(
                    "Evicted cache entry with efficiency score: {:.3}",
                    worst_score
                );
            }
        }
    }

    /// Update query patterns for prefetching
    fn update_query_patterns(&self, query_text: &str) {
        let now = Utc::now().timestamp() as u64;
        let query_lower = query_text.to_lowercase();

        // Update recent queries
        self.recent_queries
            .insert(query_lower.clone(), AtomicU64::new(now));

        // Update or create pattern
        self.patterns
            .entry(query_lower.clone())
            .and_modify(|pattern| {
                pattern.frequency.fetch_add(1, Ordering::Relaxed);
                pattern.last_seen.store(now, Ordering::Relaxed);
            })
            .or_insert_with(|| QueryPattern {
                frequency: AtomicU64::new(1),
                last_seen: AtomicU64::new(now),
            });

        // Clean up old patterns (simple implementation)
        // In production, this could be done by a background task
    }

    /// Get cache statistics snapshot
    pub fn get_stats(&self) -> QueryCacheStatsSnapshot {
        self.stats.snapshot()
    }

    /// Clear all cached entries
    pub fn clear(&self) -> Result<()> {
        let old_size = self.cache.len();
        self.cache.clear();

        self.stats.current_cache_size.store(0, Ordering::Relaxed);
        self.stats
            .estimated_memory_bytes
            .store(0, Ordering::Relaxed);

        info!("Query cache cleared ({} entries)", old_size);
        Ok(())
    }

    /// Invalidate all cache entries for a specific session
    /// More efficient than clearing the entire cache when only one session changed
    pub fn invalidate_session(&self, session_id: Uuid) -> Result<()> {
        let mut invalidated_count = 0;
        let mut keys_to_remove = Vec::new();

        // Collect keys to remove (entries associated with this session)
        for entry in self.cache.iter() {
            if entry.value().session_id == Some(session_id) {
                keys_to_remove.push(entry.key().clone());
            }
        }

        // Remove the collected keys
        for key in keys_to_remove {
            if self.cache.remove(&key).is_some() {
                invalidated_count += 1;
            }
        }

        // Update cache size
        let new_size = self.cache.len();
        self.stats
            .current_cache_size
            .store(new_size, Ordering::Relaxed);
        self.stats
            .estimated_memory_bytes
            .store(self.estimate_memory_usage(), Ordering::Relaxed);

        if invalidated_count > 0 {
            debug!(
                "Invalidated {} cache entries for session {} (remaining: {})",
                invalidated_count, session_id, new_size
            );
        }

        Ok(())
    }

    /// Estimate memory usage of the cache
    fn estimate_memory_usage(&self) -> usize {
        self.cache
            .iter()
            .map(|entry| {
                let query = entry.value();
                let text_size = query.query_text.len();
                let vector_size = query.query_vector.len() * std::mem::size_of::<f32>();
                let results_size = query.results.len() * 200; // Rough estimate per result
                text_size + vector_size + results_size + 100 // Base overhead
            })
            .sum()
    }

    /// Get cache efficiency metrics
    pub fn get_efficiency_metrics(&self) -> HashMap<String, f32> {
        let stats = self.get_stats();
        let cache_size = self.cache.len();

        let mut metrics = HashMap::new();
        metrics.insert("hit_rate".to_string(), stats.hit_rate);
        metrics.insert("avg_hit_similarity".to_string(), stats.avg_hit_similarity);
        metrics.insert(
            "cache_utilization".to_string(),
            cache_size as f32 / self.config.max_cache_size as f32 * 100.0,
        );
        metrics.insert("avg_time_saved_ms".to_string(), stats.avg_time_saved_ms);

        metrics
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new(QueryCacheConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_cache_creation() {
        let config = QueryCacheConfig::default();
        let cache = QueryCache::new(config);

        let stats = cache.get_stats();
        assert_eq!(stats.total_queries, 0);
        assert_eq!(stats.cache_hits, 0);
    }

    #[test]
    fn test_cache_and_retrieve() {
        let cache = QueryCache::default();

        let query_text = "test query".to_string();
        let query_vector = vec![0.1, 0.2, 0.3];
        let results = vec![];
        let params_hash = 12345u64;

        // Cache the results
        cache
            .cache_results(
                query_text.clone(),
                query_vector.clone(),
                results,
                params_hash,
                None,
            )
            .unwrap();

        // Try to retrieve
        let cached_results = cache.search(&query_text, &query_vector, params_hash);
        assert!(cached_results.is_some());
    }

    #[test]
    fn test_similarity_matching() {
        let cache = QueryCache::default();

        let query_vector1 = vec![1.0, 0.0, 0.0];
        let query_vector2 = vec![0.9, 0.1, 0.0]; // Similar vector
        let results = vec![];
        let params_hash = 123; // Same params_hash for both queries

        // Cache first query
        cache
            .cache_results(
                "query1".to_string(),
                query_vector1,
                results,
                params_hash,
                None,
            )
            .unwrap();

        // Search with similar vector and SAME params_hash
        // After fix: similarity matching only works if params_hash matches
        let cached_results = cache.search("query2", &query_vector2, params_hash);
        assert!(cached_results.is_some());

        // Search with different params_hash should NOT return cached results
        let different_params = cache.search("query2", &query_vector2, 456);
        assert!(
            different_params.is_none(),
            "Bug fix verification: different params_hash should not return cached results"
        );
    }

    #[test]
    fn test_cache_expiration() {
        let config = QueryCacheConfig {
            ttl_minutes: 0, // Immediate expiration
            ..Default::default()
        };

        let cache = QueryCache::new(config);

        cache
            .cache_results("test".to_string(), vec![1.0, 0.0], vec![], 123, None)
            .unwrap();

        // Should not find expired entry
        let cached_results = cache.search("test", &[1.0, 0.0], 123);
        assert!(cached_results.is_none());
    }
}

