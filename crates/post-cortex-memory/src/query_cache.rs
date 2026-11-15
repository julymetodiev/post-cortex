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

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, info};
use uuid::Uuid;

use crate::content_vectorizer::SemanticSearchResult;

mod stats;
pub use stats::{QueryCacheStats, QueryCacheStatsSnapshot};

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
    pub fn clear(&self) {
        let old_size = self.cache.len();
        self.cache.clear();

        self.stats.current_cache_size.store(0, Ordering::Relaxed);
        self.stats
            .estimated_memory_bytes
            .store(0, Ordering::Relaxed);

        info!("Query cache cleared ({} entries)", old_size);
    }

    /// Invalidate every entry whose results a write to `session_id` can
    /// change: that session's entries, multi-session entries including it,
    /// and all global entries.
    pub fn invalidate_session(&self, session_id: Uuid) {
        let removed = self.remove_where(|entry| entry.scope.covers(session_id));
        if removed > 0 {
            debug!(
                "Invalidated {} cache entries for session {} (remaining: {})",
                removed,
                session_id,
                self.cache.len()
            );
        }
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
mod tests;
