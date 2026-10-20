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
//! Entries are keyed by the hash of the full query parameters (query text,
//! limit, scope, date range, recency bias), so a lookup is one map probe and
//! happens *before* the query is embedded. Results are shared as
//! `Arc<[SemanticSearchResult]>`: a hit never copies them under the map lock.
//!
//! Every entry records the sessions it covers ([`CacheScope`]). A write to a
//! session must call [`QueryCache::invalidate_session`], which drops that
//! session's entries, every multi-session entry that includes it and every
//! global entry — otherwise those would serve stale results until the TTL.

use anyhow::Result;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, info};
use uuid::Uuid;

use crate::content_vectorizer::SemanticSearchResult;

/// Run a full expiry sweep at most once per this many inserts.
const EXPIRY_SWEEP_INTERVAL: u64 = 64;

/// Configuration for query cache
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryCacheConfig {
    /// Maximum number of cached queries
    pub max_cache_size: usize,
    /// Time-to-live for cached results in minutes
    pub ttl_minutes: i64,
    /// Ignored. Lookups are exact on the query parameters; kept so existing
    /// configs still deserialize.
    pub similarity_threshold: f32,
    /// Ignored (prefetching was never implemented); kept for config compatibility.
    pub enable_prefetching: bool,
    /// Ignored (prefetching was never implemented); kept for config compatibility.
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

/// Which sessions a cached result was computed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheScope {
    /// Searched every session: any write invalidates it.
    Global,
    /// Searched one session.
    Session(Uuid),
    /// Searched exactly these sessions.
    Sessions(Arc<[Uuid]>),
}

impl CacheScope {
    /// Whether a write to `session_id` can change this entry's results.
    pub fn covers(&self, session_id: Uuid) -> bool {
        match self {
            Self::Global => true,
            Self::Session(id) => *id == session_id,
            Self::Sessions(ids) => ids.contains(&session_id),
        }
    }
}

/// A cached query result with atomic access tracking
#[derive(Debug)]
pub struct CachedQuery {
    /// Original query text (guards against hash collisions)
    pub query_text: String,
    /// Cached search results, shared with every hit
    pub results: Arc<[SemanticSearchResult]>,
    /// Sessions the results were computed from
    pub scope: CacheScope,
    /// Query parameters hash (the map key)
    pub params_hash: u64,
    cached_at: Instant,
    access_count: AtomicU64,
    estimated_bytes: usize,
}

impl CachedQuery {
    /// Create a new cached query
    pub fn new(
        query_text: String,
        results: Arc<[SemanticSearchResult]>,
        params_hash: u64,
        scope: CacheScope,
    ) -> Self {
        let estimated_bytes = estimate_entry_bytes(&query_text, &results);
        Self {
            query_text,
            results,
            scope,
            params_hash,
            cached_at: Instant::now(),
            access_count: AtomicU64::new(0),
            estimated_bytes,
        }
    }

    /// Check if the cached query has expired
    pub fn is_expired(&self, ttl_minutes: i64) -> bool {
        let ttl = Duration::from_secs(u64::try_from(ttl_minutes.max(0)).unwrap_or(0) * 60);
        self.cached_at.elapsed() >= ttl
    }

    /// Record one access
    pub fn mark_accessed(&self) {
        self.access_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of times this entry was served
    pub fn access_count(&self) -> u64 {
        self.access_count.load(Ordering::Relaxed)
    }

    /// Eviction score: frequently used, recent entries score higher.
    pub fn efficiency_score(&self) -> f32 {
        let hours = (self.cached_at.elapsed().as_secs() / 3600).max(1) as f32;
        let recency_factor = 1.0 / (1.0 + hours / 24.0);
        let frequency_factor = (self.access_count() as f32).ln().max(1.0);
        recency_factor * frequency_factor
    }
}

/// Rough size of one entry (text + per-result estimate + overhead).
fn estimate_entry_bytes(query_text: &str, results: &[SemanticSearchResult]) -> usize {
    let results_bytes: usize = results
        .iter()
        .map(|r| {
            std::mem::size_of::<SemanticSearchResult>() + r.content_id.len() + r.text_content.len()
        })
        .sum();
    query_text.len() + results_bytes + std::mem::size_of::<CachedQuery>()
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
    /// Current cache size (atomic)
    pub current_cache_size: AtomicUsize,
    /// Memory usage estimation in bytes (atomic)
    pub estimated_memory_bytes: AtomicUsize,
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
            current_cache_size: AtomicUsize::new(0),
            estimated_memory_bytes: AtomicUsize::new(0),
        }
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
            // Hits are exact parameter matches.
            avg_hit_similarity: if hits > 0 { 1.0 } else { 0.0 },
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
    /// Total queries processed since last reset.
    pub total_queries: u64,
    /// Number of cache hits.
    pub cache_hits: u64,
    /// Number of cache misses.
    pub cache_misses: u64,
    /// Expired entries removed by cleanup.
    pub expired_removed: u64,
    /// Entries evicted due to capacity pressure.
    pub evicted_entries: u64,
    /// Average similarity score of cache hits.
    pub avg_hit_similarity: f32,
    /// Number of entries currently in the cache.
    pub current_cache_size: usize,
    /// Estimated memory consumption in bytes.
    pub estimated_memory_bytes: usize,
    /// Cache hit rate as a percentage (0–100).
    pub hit_rate: f32,
    /// Average wall-clock time saved per cache hit in milliseconds.
    pub avg_time_saved_ms: f32,
}

/// Query cache using DashMap and atomic operations
pub struct QueryCache {
    /// Entries keyed by query-parameter hash
    cache: DashMap<u64, Arc<CachedQuery>>,

    /// Cache configuration - immutable after creation
    config: QueryCacheConfig,

    /// Performance statistics using atomics
    stats: QueryCacheStats,

    /// Inserts since creation; drives the periodic expiry sweep
    inserts: AtomicU64,
}

impl QueryCache {
    /// Create a new query cache
    pub fn new(config: QueryCacheConfig) -> Self {
        info!(
            "Initializing query cache with max size: {}",
            config.max_cache_size
        );

        Self {
            cache: DashMap::new(),
            config,
            stats: QueryCacheStats::new(),
            inserts: AtomicU64::new(0),
        }
    }

    /// Look up cached results by parameter hash. O(1); an expired entry is
    /// removed and reported as a miss.
    pub fn get(&self, params_hash: u64, query_text: &str) -> Option<Arc<[SemanticSearchResult]>> {
        self.stats.total_queries.fetch_add(1, Ordering::Relaxed);

        let entry = self.cache.get(&params_hash).map(|e| Arc::clone(e.value()));
        let hit = match entry {
            Some(entry) if entry.is_expired(self.config.ttl_minutes) => {
                self.remove_if_same(params_hash, &entry, true);
                None
            }
            Some(entry) if entry.query_text == query_text => {
                entry.mark_accessed();
                Some(Arc::clone(&entry.results))
            }
            _ => None,
        };

        let counter = if hit.is_some() {
            &self.stats.cache_hits
        } else {
            &self.stats.cache_misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
        hit
    }

    /// Cache query results
    pub fn insert(
        &self,
        query_text: String,
        results: Arc<[SemanticSearchResult]>,
        params_hash: u64,
        scope: CacheScope,
    ) {
        let inserts = self.inserts.fetch_add(1, Ordering::Relaxed) + 1;
        if inserts.is_multiple_of(EXPIRY_SWEEP_INTERVAL) {
            self.cleanup_expired();
        }

        if self.cache.len() >= self.config.max_cache_size.max(1)
            && !self.cache.contains_key(&params_hash)
        {
            self.cleanup_expired();
            if self.cache.len() >= self.config.max_cache_size.max(1) {
                self.evict_least_efficient();
            }
        }

        let entry = Arc::new(CachedQuery::new(query_text, results, params_hash, scope));
        let added = entry.estimated_bytes;
        if let Some(old) = self.cache.insert(params_hash, entry) {
            self.sub_bytes(old.estimated_bytes);
        }
        self.stats
            .estimated_memory_bytes
            .fetch_add(added, Ordering::Relaxed);
        self.sync_size();
    }

    /// Remove `params_hash` only if it still maps to `entry` (a concurrent
    /// insert may have replaced it).
    fn remove_if_same(&self, params_hash: u64, entry: &Arc<CachedQuery>, expired: bool) {
        if let Some((_, removed)) = self
            .cache
            .remove_if(&params_hash, |_, v| Arc::ptr_eq(v, entry))
        {
            self.account_removed(&removed);
            if expired {
                self.stats.expired_removed.fetch_add(1, Ordering::Relaxed);
            }
            self.sync_size();
        }
    }

    fn account_removed(&self, removed: &CachedQuery) {
        self.sub_bytes(removed.estimated_bytes);
    }

    /// Saturating: a concurrent `clear` may already have reset the counter.
    fn sub_bytes(&self, bytes: usize) {
        let counter = &self.stats.estimated_memory_bytes;
        let mut current = counter.load(Ordering::Relaxed);
        while let Err(actual) = counter.compare_exchange_weak(
            current,
            current.saturating_sub(bytes),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            current = actual;
        }
    }

    fn sync_size(&self) {
        self.stats
            .current_cache_size
            .store(self.cache.len(), Ordering::Relaxed);
    }

    /// Remove every entry matching `predicate`; returns how many went.
    fn remove_where(&self, predicate: impl Fn(&CachedQuery) -> bool) -> usize {
        let mut removed = 0usize;
        let mut freed = 0usize;
        self.cache.retain(|_, entry| {
            if predicate(entry) {
                removed += 1;
                freed += entry.estimated_bytes;
                false
            } else {
                true
            }
        });
        if removed > 0 {
            self.sub_bytes(freed);
            self.sync_size();
        }
        removed
    }

    /// Clean up expired cache entries
    fn cleanup_expired(&self) {
        let ttl = self.config.ttl_minutes;
        let removed = self.remove_where(|entry| entry.is_expired(ttl));
        if removed > 0 {
            self.stats
                .expired_removed
                .fetch_add(removed as u64, Ordering::Relaxed);
            debug!("Removed {} expired cache entries", removed);
        }
    }

    /// Evict the least efficient ~10% of the cache in one pass, so inserts
    /// at capacity pay for a scan once per many inserts, not every time.
    fn evict_least_efficient(&self) {
        let batch = (self.config.max_cache_size / 10).max(1);
        let mut scored: Vec<(f32, u64, Arc<CachedQuery>)> = self
            .cache
            .iter()
            .map(|e| {
                (
                    e.value().efficiency_score(),
                    *e.key(),
                    Arc::clone(e.value()),
                )
            })
            .collect();
        if scored.len() > batch {
            scored.select_nth_unstable_by(batch - 1, |a, b| a.0.total_cmp(&b.0));
            scored.truncate(batch);
        }

        let mut evicted = 0u64;
        for (_, key, entry) in scored {
            if let Some((_, removed)) = self.cache.remove_if(&key, |_, v| Arc::ptr_eq(v, &entry)) {
                self.account_removed(&removed);
                evicted += 1;
            }
        }
        if evicted > 0 {
            self.stats
                .evicted_entries
                .fetch_add(evicted, Ordering::Relaxed);
            self.sync_size();
            debug!("Evicted {} query cache entries", evicted);
        }
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

    /// Invalidate every entry whose results a write to `session_id` can
    /// change: that session's entries, multi-session entries including it,
    /// and all global entries.
    pub fn invalidate_session(&self, session_id: Uuid) -> Result<()> {
        let removed = self.remove_where(|entry| entry.scope.covers(session_id));
        if removed > 0 {
            debug!(
                "Invalidated {} cache entries for session {} (remaining: {})",
                removed,
                session_id,
                self.cache.len()
            );
        }
        Ok(())
    }

    /// Number of cached entries
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
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
    use crate::content_vectorizer::ContentType;
    use chrono::Utc;

    fn result(session_id: Uuid, text: &str) -> SemanticSearchResult {
        SemanticSearchResult {
            content_id: Uuid::new_v4().to_string(),
            session_id,
            content_type: ContentType::UpdateContent,
            text_content: text.to_string(),
            similarity_score: 0.9,
            importance_score: 0.7,
            timestamp: Utc::now(),
            combined_score: 0.84,
            neighbour_of: None,
            author: None,
            derived_from: Vec::new(),
        }
    }

    fn empty() -> Arc<[SemanticSearchResult]> {
        Arc::from(Vec::new())
    }

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
        let sid = Uuid::new_v4();
        let results: Arc<[SemanticSearchResult]> = Arc::from(vec![result(sid, "hello")]);

        cache.insert(
            "test query".into(),
            Arc::clone(&results),
            12345,
            CacheScope::Global,
        );

        let hit = cache.get(12345, "test query").expect("hit");
        // Shared, not copied.
        assert!(Arc::ptr_eq(&hit, &results));
        let stats = cache.get_stats();
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.total_queries, 1);
    }

    #[test]
    fn test_different_params_miss() {
        let cache = QueryCache::default();
        cache.insert("query".into(), empty(), 123, CacheScope::Global);
        assert!(cache.get(456, "query").is_none());
        // Same hash but different text (collision guard) misses too.
        assert!(cache.get(123, "other").is_none());
        assert_eq!(cache.get_stats().cache_misses, 2);
    }

    #[test]
    fn test_cache_expiration() {
        let config = QueryCacheConfig {
            ttl_minutes: 0, // Immediate expiration
            ..Default::default()
        };

        let cache = QueryCache::new(config);
        cache.insert("test".into(), empty(), 123, CacheScope::Global);

        // Should not find expired entry, and the lookup removes it.
        assert!(cache.get(123, "test").is_none());
        assert!(cache.is_empty());
        assert_eq!(cache.get_stats().expired_removed, 1);
    }

    #[test]
    fn test_invalidate_session_drops_covering_scopes() {
        let cache = QueryCache::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        cache.insert("q".into(), empty(), 1, CacheScope::Session(a));
        cache.insert("q".into(), empty(), 2, CacheScope::Session(b));
        cache.insert("q".into(), empty(), 3, CacheScope::Global);
        cache.insert(
            "q".into(),
            empty(),
            4,
            CacheScope::Sessions(Arc::from(vec![a, b])),
        );
        cache.insert(
            "q".into(),
            empty(),
            5,
            CacheScope::Sessions(Arc::from(vec![b])),
        );

        cache.invalidate_session(a).unwrap();

        assert!(cache.get(1, "q").is_none(), "own session entry");
        assert!(
            cache.get(3, "q").is_none(),
            "global entry must not go stale"
        );
        assert!(cache.get(4, "q").is_none(), "multi-session entry with a");
        assert!(cache.get(2, "q").is_some());
        assert!(cache.get(5, "q").is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_capacity_is_enforced() {
        let cache = QueryCache::new(QueryCacheConfig {
            max_cache_size: 20,
            ..Default::default()
        });
        for i in 0..100u64 {
            cache.insert(format!("q{i}"), empty(), i, CacheScope::Global);
            assert!(cache.len() <= 20);
        }
        assert!(cache.get_stats().evicted_entries > 0);
        // The newest entry always survives its own insert.
        assert!(cache.get(99, "q99").is_some());
    }

    #[test]
    fn test_replacing_key_does_not_evict() {
        let cache = QueryCache::new(QueryCacheConfig {
            max_cache_size: 2,
            ..Default::default()
        });
        cache.insert("a".into(), empty(), 1, CacheScope::Global);
        cache.insert("b".into(), empty(), 2, CacheScope::Global);
        cache.insert("b".into(), empty(), 2, CacheScope::Global);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get_stats().evicted_entries, 0);
    }

    #[test]
    fn test_memory_estimate_is_incremental() {
        let cache = QueryCache::default();
        let sid = Uuid::new_v4();
        let results: Arc<[SemanticSearchResult]> = Arc::from(vec![result(sid, "some text")]);

        cache.insert(
            "q".into(),
            Arc::clone(&results),
            1,
            CacheScope::Session(sid),
        );
        let one = cache.get_stats().estimated_memory_bytes;
        assert!(one > 0);

        cache.insert(
            "q".into(),
            Arc::clone(&results),
            1,
            CacheScope::Session(sid),
        );
        assert_eq!(cache.get_stats().estimated_memory_bytes, one, "replace");

        cache.insert("q".into(), results, 2, CacheScope::Session(sid));
        assert_eq!(cache.get_stats().estimated_memory_bytes, 2 * one);

        cache.invalidate_session(sid).unwrap();
        assert_eq!(cache.get_stats().estimated_memory_bytes, 0);
        assert_eq!(cache.get_stats().current_cache_size, 0);
    }
}
