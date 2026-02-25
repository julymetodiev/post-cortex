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
use atomic_float::AtomicF64;
use crossbeam_channel::{Receiver, Sender, bounded};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// LRU-like cache using `DashMap`
/// Uses atomic counters for LRU approximation and metrics
pub struct Cache<K, V> {
    /// Main storage - completely lock-free
    data: DashMap<K, CacheEntry<V>>,

    /// Cache configuration - atomic
    capacity: AtomicUsize,
    current_size: AtomicUsize,

    /// LRU approximation using access counters
    global_access_counter: AtomicU64,

    /// Metrics - all atomic
    total_requests: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    total_lookup_time_ns: AtomicU64,

    /// Cached computed values for performance
    hit_rate: AtomicF64,
    avg_lookup_time_ns: AtomicU64,

    /// Cache metadata
    name: String,
    created_at: AtomicU64,

    /// Eviction channel for async processing (optional)
    eviction_sender: Sender<EvictionEvent<K>>,
    eviction_receiver: Receiver<EvictionEvent<K>>,
}

/// Cache entry with access tracking for LRU approximation
#[derive(Debug)]
struct CacheEntry<V> {
    value: V,
    access_count: AtomicU64,
    last_accessed: AtomicU64,
    #[allow(dead_code)]
    created_at: AtomicU64,
}

/// Event for async eviction processing
#[derive(Debug, Clone)]
pub enum EvictionEvent<K> {
    #[allow(dead_code)]
    ShouldEvict {
        key: K,
        access_count: u64,
        last_accessed: u64,
    },
    Evicted {
        #[allow(dead_code)]
        key: K,
        #[allow(dead_code)]
        timestamp: u64,
    },
}

/// Cache statistics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub name: String,
    pub current_size: usize,
    pub capacity: usize,
    pub utilization: f64,
    pub total_requests: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub hit_rate: f64,
    pub miss_rate: f64,
    pub avg_lookup_time_ns: u64,
    pub created_at: u64,
    pub uptime_seconds: u64,
}

impl<V> CacheEntry<V> {
    /// Creates a new cache entry with the given logical timestamp
    fn new_with_logical_time(value: V, logical_time: u64) -> Self {
        Self {
            value,
            access_count: AtomicU64::new(1),
            last_accessed: AtomicU64::new(logical_time),
            created_at: AtomicU64::new(logical_time),
        }
    }

    /// Updates access tracking using logical timestamp (no syscall)
    fn touch(&self, global_counter: &AtomicU64) -> u64 {
        // Use global counter as logical timestamp (Lamport clock)
        // This eliminates SystemTime syscall overhead (~20-100ns per call)
        let logical_time = global_counter.fetch_add(1, Ordering::Relaxed) + 1;
        self.last_accessed.store(logical_time, Ordering::Relaxed);
        self.access_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Calculates eviction priority using logical timestamps (no syscall)
    /// Higher score = higher eviction priority (should be evicted first)
    fn priority_score(&self, current_logical_time: u64) -> u64 {
        let access_count = self.access_count.load(Ordering::Relaxed);
        let last_accessed = self.last_accessed.load(Ordering::Relaxed);

        // Recency: how many logical ticks since last access
        let recency_factor = current_logical_time.saturating_sub(last_accessed);

        // Frequency: inverse of access count (less accessed = higher priority)
        let frequency_factor = if access_count > 0 {
            1000 / access_count
        } else {
            1000
        };

        // Higher recency + lower frequency = higher eviction priority
        recency_factor + frequency_factor
    }
}

impl<K, V> Cache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Creates a new cache with the specified capacity and name
    ///
    /// # Errors
    /// Returns an error if capacity is 0
    ///
    /// # Panics
    /// Panics if the system time is before `UNIX_EPOCH` (should never happen in practice)
    pub fn new(capacity: usize, name: String) -> Result<Self, String> {
        if capacity == 0 {
            return Err("Cache capacity must be greater than 0".to_string());
        }

        let (eviction_sender, eviction_receiver) = bounded(1000);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Ok(Self {
            data: DashMap::new(),
            capacity: AtomicUsize::new(capacity),
            current_size: AtomicUsize::new(0),
            global_access_counter: AtomicU64::new(0),
            total_requests: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            total_lookup_time_ns: AtomicU64::new(0),
            hit_rate: AtomicF64::new(0.0),
            avg_lookup_time_ns: AtomicU64::new(0),
            name,
            created_at: AtomicU64::new(now),
            eviction_sender,
            eviction_receiver,
        })
    }

    /// Get value from cache - completely lock-free
    #[allow(clippy::option_if_let_else)]
    pub fn get(&self, key: &K) -> Option<V> {
        let start = Instant::now();
        let total_requests = self.total_requests.fetch_add(1, Ordering::Relaxed) + 1;

        let result = if let Some(entry_ref) = self.data.get(key) {
            // Cache hit - update access tracking
            let access_count = entry_ref.value().touch(&self.global_access_counter);
            let value = entry_ref.value().value.clone();

            // Update hit metrics
            let hits = self.hits.fetch_add(1, Ordering::Relaxed) + 1;

            // Update cached hit rate
            #[allow(clippy::cast_precision_loss)]
            let hit_rate = hits as f64 / total_requests as f64;
            self.hit_rate.store(hit_rate, Ordering::Relaxed);

            debug!(
                "{} Cache: HIT (access_count: {})",
                self.name, access_count
            );
            Some(value)
        } else {
            // Cache miss
            self.misses.fetch_add(1, Ordering::Relaxed);

            // Update cached hit rate
            let hits = self.hits.load(Ordering::Relaxed);
            #[allow(clippy::cast_precision_loss)]
            let hit_rate = hits as f64 / total_requests as f64;
            self.hit_rate.store(hit_rate, Ordering::Relaxed);

            debug!("{} Cache: MISS", self.name);
            None
        };

        // Update lookup time metrics
        #[allow(clippy::cast_possible_truncation)]
        let lookup_time_ns = start.elapsed().as_nanos() as u64;
        let total_lookup_time = self
            .total_lookup_time_ns
            .fetch_add(lookup_time_ns, Ordering::Relaxed)
            + lookup_time_ns;
        let avg_lookup_time = total_lookup_time / total_requests;
        self.avg_lookup_time_ns
            .store(avg_lookup_time, Ordering::Relaxed);

        result
    }

    /// Put value in cache - lock-free with atomic eviction
    pub fn put(&self, key: K, value: V) -> Option<V> {
        let capacity = self.capacity.load(Ordering::Relaxed);
        let logical_time = self.global_access_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = CacheEntry::new_with_logical_time(value, logical_time);

        // Check if we need to evict
        let current_size = self.current_size.load(Ordering::Relaxed);
        if current_size >= capacity {
            self.try_evict();
        }

        // Insert new entry
        let old_value = self.data.insert(key, entry).map(|old_entry| {
            // Key existed - this is a replacement, not size increase
            old_entry.value
        });

        if old_value.is_none() {
            // New key - increment size
            self.current_size.fetch_add(1, Ordering::Relaxed);
        }

        old_value
    }

    /// Approximate LRU eviction - completely lock-free
    fn try_evict(&self) {
        let capacity = self.capacity.load(Ordering::Relaxed);
        let current_size = self.current_size.load(Ordering::Relaxed);

        if current_size < capacity {
            return; // Race condition - size decreased, no need to evict
        }

        // Get current logical time for priority calculation (no syscall)
        let current_logical_time = self.global_access_counter.load(Ordering::Relaxed);

        // Find candidate for eviction by scanning entries
        // This is O(n) but only happens on capacity overflow
        let mut eviction_candidate: Option<(K, u64)> = None;
        let mut highest_eviction_priority = 0;

        // Sample a subset of entries for efficiency (approximate LRU)
        let sample_size = std::cmp::min(20, self.data.len());
        for (sampled, entry_ref) in self.data.iter().enumerate() {
            if sampled >= sample_size {
                break;
            }

            let priority = entry_ref.value().priority_score(current_logical_time);
            if priority > highest_eviction_priority {
                highest_eviction_priority = priority;
                eviction_candidate = Some((entry_ref.key().clone(), priority));
            }
        }

        // Evict the candidate
        if let Some((key, _priority)) = eviction_candidate
            && let Some((_key, _old_entry)) = self.data.remove(&key)
        {
            self.current_size.fetch_sub(1, Ordering::Relaxed);
            self.evictions.fetch_add(1, Ordering::Relaxed);

            // Send eviction event (non-blocking)
            let _ = self.eviction_sender.try_send(EvictionEvent::Evicted {
                key,
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            });

            debug!("{} Cache: Evicted entry", self.name);
        }
    }

    /// Check if key exists - lock-free
    pub fn contains_key(&self, key: &K) -> bool {
        self.data.contains_key(key)
    }

    /// Remove specific key - lock-free
    pub fn remove(&self, key: &K) -> Option<V> {
        if let Some((_key, entry)) = self.data.remove(key) {
            self.current_size.fetch_sub(1, Ordering::Relaxed);
            Some(entry.value)
        } else {
            None
        }
    }

    /// Clear all entries - lock-free
    pub fn clear(&self) {
        let old_size = self.current_size.load(Ordering::Relaxed);
        self.data.clear();
        self.current_size.store(0, Ordering::Relaxed);

        if old_size > 0 {
            info!("{} Cache: Cleared {} entries", self.name, old_size);
        }
    }

    /// Get current size - atomic read
    pub fn len(&self) -> usize {
        self.current_size.load(Ordering::Relaxed)
    }

    /// Check if empty - atomic read
    pub fn is_empty(&self) -> bool {
        self.current_size.load(Ordering::Relaxed) == 0
    }

    /// Get capacity - atomic read
    pub fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Get cache statistics - all atomic reads
    /// Gets comprehensive statistics about the cache
    ///
    /// # Panics
    /// Panics if the system time is before `UNIX_EPOCH` (should never happen in practice)
    pub fn get_stats(&self) -> CacheStats {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let created_at = self.created_at.load(Ordering::Relaxed);
        let current_size = self.current_size.load(Ordering::Relaxed);
        let capacity = self.capacity.load(Ordering::Relaxed);

        CacheStats {
            name: self.name.clone(),
            current_size,
            capacity,
            utilization: if capacity > 0 {
                #[allow(clippy::cast_precision_loss)]
                {
                    current_size as f64 / capacity as f64
                }
            } else {
                0.0
            },
            total_requests: self.total_requests.load(Ordering::Relaxed),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            hit_rate: self.hit_rate.load(Ordering::Relaxed),
            miss_rate: 1.0 - self.hit_rate.load(Ordering::Relaxed),
            avg_lookup_time_ns: self.avg_lookup_time_ns.load(Ordering::Relaxed),
            created_at,
            uptime_seconds: now.saturating_sub(created_at),
        }
    }

    /// Reset metrics - atomic operations
    pub fn reset_metrics(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.total_lookup_time_ns.store(0, Ordering::Relaxed);
        self.hit_rate.store(0.0, Ordering::Relaxed);
        self.avg_lookup_time_ns.store(0, Ordering::Relaxed);

        info!("{} Cache: Metrics reset", self.name);
    }

    /// Resize cache capacity - atomic
    /// Resizes the cache to a new capacity
    ///
    /// # Errors
    /// Returns an error if `new_capacity` is 0 or if eviction fails during resize
    pub fn resize(&self, new_capacity: usize) -> Result<(), String> {
        if new_capacity == 0 {
            return Err("Cache capacity must be greater than 0".to_string());
        }

        let old_capacity = self.capacity.swap(new_capacity, Ordering::Relaxed);
        let current_size = self.current_size.load(Ordering::Relaxed);

        // If new capacity is smaller, trigger evictions
        if new_capacity < current_size {
            let evictions_needed = current_size - new_capacity;
            for _ in 0..evictions_needed {
                self.try_evict();
            }
        }

        info!(
            "{} Cache: Resized from {} to {} capacity",
            self.name, old_capacity, new_capacity
        );

        Ok(())
    }

    /// Check for performance issues - atomic reads only
    pub fn has_performance_issues(&self) -> bool {
        let stats = self.get_stats();

        // Low hit rate with significant usage
        if stats.hit_rate < 0.3 && stats.total_requests > 100 {
            return true;
        }

        // Very slow lookups (> 1ms average)
        if stats.avg_lookup_time_ns > 1_000_000 {
            return true;
        }

        // High eviction rate (> 50% of requests)
        let eviction_rate = if stats.total_requests > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                stats.evictions as f64 / stats.total_requests as f64
            }
        } else {
            0.0
        };

        if eviction_rate > 0.5 {
            return true;
        }

        false
    }

    /// Get performance recommendations
    pub fn get_recommendations(&self) -> Vec<String> {
        let mut recommendations = Vec::new();
        let stats = self.get_stats();

        if stats.hit_rate < 0.5 && stats.total_requests > 100 {
            recommendations.push(format!(
                "Consider increasing cache size. Current hit rate: {:.1}%",
                stats.hit_rate * 100.0
            ));
        }

        if stats.utilization > 0.9 {
            recommendations.push("Cache is nearly full, consider increasing capacity".to_string());
        }

        if stats.avg_lookup_time_ns > 500_000 {
            recommendations.push(format!(
                "Slow cache lookups detected: {}µs average",
                stats.avg_lookup_time_ns / 1000
            ));
        }

        let eviction_rate = if stats.total_requests > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                stats.evictions as f64 / stats.total_requests as f64
            }
        } else {
            0.0
        };

        if eviction_rate > 0.2 {
            recommendations.push(format!(
                "High eviction rate: {:.1}%, consider increasing capacity",
                eviction_rate * 100.0
            ));
        }

        recommendations
    }

    /// Get all keys - creates snapshot (potentially expensive)
    pub fn keys(&self) -> Vec<K> {
        self.data.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Get all values - creates snapshot (potentially expensive)
    pub fn values(&self) -> Vec<V> {
        self.data
            .iter()
            .map(|entry| entry.value().value.clone())
            .collect()
    }

    /// Iterate over entries with callback - lock-free iteration
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&K, &V),
    {
        for entry_ref in &self.data {
            f(entry_ref.key(), &entry_ref.value().value);
        }
    }

    /// Drain eviction events (for monitoring)
    pub fn drain_eviction_events(&self) -> Vec<EvictionEvent<K>> {
        let mut events = Vec::new();
        while let Ok(event) = self.eviction_receiver.try_recv() {
            events.push(event);
        }
        events
    }
}

// Note: Clone implementation removed due to thread safety complexity
// Use cache.keys() and cache.values() for manual copying if needed

/// Type alias for session cache
pub type SessionCache<K, V> = Cache<K, V>;

/// Multi-cache manager using lock-free caches
pub struct CacheManager {
    caches: DashMap<String, Arc<dyn CacheProvider + Send + Sync>>,
    total_requests: AtomicU64,
    total_hits: AtomicU64,
    total_misses: AtomicU64,
    total_evictions: AtomicU64,
}

/// Trait for generic cache operations
pub trait CacheProvider {
    fn get_stats(&self) -> CacheStats;
    fn reset_metrics(&self);
    fn has_performance_issues(&self) -> bool;
    fn get_recommendations(&self) -> Vec<String>;
}

impl<K, V> CacheProvider for Cache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn get_stats(&self) -> CacheStats {
        self.get_stats()
    }

    fn reset_metrics(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.total_lookup_time_ns.store(0, Ordering::Relaxed);
        self.hit_rate.store(0.0, Ordering::Relaxed);
        self.avg_lookup_time_ns.store(0, Ordering::Relaxed);
    }

    fn has_performance_issues(&self) -> bool {
        self.has_performance_issues()
    }

    fn get_recommendations(&self) -> Vec<String> {
        self.get_recommendations()
    }
}

impl CacheManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            total_requests: AtomicU64::new(0),
            total_hits: AtomicU64::new(0),
            total_misses: AtomicU64::new(0),
            total_evictions: AtomicU64::new(0),
        }
    }

    pub fn register_cache<K, V>(&self, name: &str, cache: Cache<K, V>)
    where
        K: Hash + Eq + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        self.caches.insert(name.to_string(), Arc::new(cache));
        info!("Registered cache: {}", name);
    }

    pub fn get_all_stats(&self) -> Vec<CacheStats> {
        self.caches
            .iter()
            .map(|entry| entry.value().get_stats())
            .collect()
    }

    pub fn has_any_performance_issues(&self) -> bool {
        self.caches
            .iter()
            .any(|entry| entry.value().has_performance_issues())
    }

    pub fn reset_all_metrics(&self) {
        for entry in &self.caches {
            entry.value().reset_metrics();
        }

        self.total_requests.store(0, Ordering::Relaxed);
        self.total_hits.store(0, Ordering::Relaxed);
        self.total_misses.store(0, Ordering::Relaxed);
        self.total_evictions.store(0, Ordering::Relaxed);

        info!("Reset all cache metrics");
    }

    pub fn get_summary(&self) -> CacheManagerSummary {
        let stats: Vec<CacheStats> = self.get_all_stats();
        let cache_count = stats.len();

        let total_requests: u64 = stats.iter().map(|s| s.total_requests).sum();
        let total_hits: u64 = stats.iter().map(|s| s.hits).sum();
        let total_evictions: u64 = stats.iter().map(|s| s.evictions).sum();

        let avg_hit_rate = if total_requests > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                total_hits as f64 / total_requests as f64
            }
        } else {
            0.0
        };

        let problematic_caches: Vec<String> = self
            .caches
            .iter()
            .filter_map(|entry| {
                if entry.value().has_performance_issues() {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        CacheManagerSummary {
            cache_count,
            total_requests,
            total_hits,
            total_evictions,
            average_hit_rate: avg_hit_rate,
            problematic_caches,
            individual_stats: stats,
        }
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheManagerSummary {
    pub cache_count: usize,
    pub total_requests: u64,
    pub total_hits: u64,
    pub total_evictions: u64,
    pub average_hit_rate: f64,
    pub problematic_caches: Vec<String>,
    pub individual_stats: Vec<CacheStats>,
}

// Global cache manager
static GLOBAL_CACHE_MANAGER: std::sync::OnceLock<CacheManager> =
    std::sync::OnceLock::new();

pub fn init_global_cache_manager() {
    let _ = GLOBAL_CACHE_MANAGER.set(CacheManager::new());
}

pub fn get_global_cache_manager() -> &'static CacheManager {
    GLOBAL_CACHE_MANAGER.get_or_init(CacheManager::new)
}

/// Convenience macro for creating a cache
#[macro_export]
macro_rules! new_cache {
    ($capacity:expr, $name:expr) => {
        $crate::core::cache::Cache::new($capacity, $name.to_string())
    };
}
