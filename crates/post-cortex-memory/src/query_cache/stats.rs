// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Counters of the query cache and their snapshot.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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
