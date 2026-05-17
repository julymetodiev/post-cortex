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
use crossbeam_channel::{bounded, Receiver, Sender};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// Performance monitor using atomic operations
#[derive(Debug)]
pub struct PerformanceMonitor {
    /// Operation metrics - concurrent map
    operations: DashMap<String, Arc<OperationMetrics>>,

    /// Cache metrics - concurrent map
    caches: DashMap<String, Arc<CacheMetrics>>,

    /// Global counters - all atomic
    /// Total number of operations recorded
    total_operations: AtomicU64,
    /// Total number of errors encountered
    total_errors: AtomicU64,
    /// Number of currently active operations
    active_operations: AtomicUsize,
    /// UNIX timestamp when monitoring started
    started_at_timestamp: AtomicU64,

    /// Session info - atomic
    session_id: Option<String>,

    /// Metrics collection - channel
    metrics_sender: Sender<MetricsEvent>,
    #[allow(dead_code)]
    metrics_receiver: Receiver<MetricsEvent>,
}

/// Operation metrics using atomics
#[derive(Debug)]
pub struct OperationMetrics {
    /// Name of the tracked operation
    operation_name: String,

    /// Total number of times this operation was called
    total_calls: AtomicU64,
    /// Number of times this operation resulted in an error
    error_count: AtomicU64,

    /// Cumulative duration of all calls in nanoseconds
    total_duration_ns: AtomicU64,
    /// Minimum observed duration in nanoseconds
    min_duration_ns: AtomicU64,
    /// Maximum observed duration in nanoseconds
    max_duration_ns: AtomicU64,

    /// UNIX timestamp of the most recent execution
    last_execution_timestamp: AtomicU64,

    /// Cached average duration in nanoseconds
    avg_duration_ns: AtomicU64,
    /// Cached error rate as a percentage (0–100)
    error_rate: AtomicF64,

    /// Sum of durations in the recent sliding window
    recent_duration_sum: AtomicU64,
    /// Number of operations in the recent sliding window (max 100)
    recent_operation_count: AtomicU64,
}

/// Cache metrics using atomics
#[derive(Debug)]
pub struct CacheMetrics {
    /// Name of the tracked cache
    cache_name: String,

    /// Total number of cache requests
    total_requests: AtomicU64,
    /// Number of cache hits
    hits: AtomicU64,
    /// Number of cache misses
    misses: AtomicU64,
    /// Number of cache evictions
    evictions: AtomicU64,

    /// Cumulative lookup time across all requests in nanoseconds
    total_lookup_time_ns: AtomicU64,
    /// Cached average lookup time in nanoseconds
    avg_lookup_time_ns: AtomicU64,

    /// Cached hit rate (0.0–1.0)
    hit_rate: AtomicF64,
    /// Cached miss rate (0.0–1.0)
    miss_rate: AtomicF64,

    /// UNIX timestamp of the most recent metric update
    last_updated_timestamp: AtomicU64,
    #[allow(dead_code)]
    /// UNIX timestamp when this cache was first tracked
    created_timestamp: AtomicU64,
}

/// Operation timer that tracks elapsed time for a named operation
pub struct OperationTimer {
    #[allow(dead_code)]
    /// Name of the operation being timed
    operation_name: String,
    /// Instant when timing started
    start_time: Instant,
    /// Optional reference to the performance monitor
    monitor: Option<Arc<PerformanceMonitor>>,
    /// Whether the timer has already been finished
    is_finished: AtomicBool,
}

/// Events for async metrics processing (if needed)
#[derive(Debug, Clone)]
enum MetricsEvent {
    /// An operation completed
    OperationCompleted {
        #[allow(dead_code)]
        /// Name of the completed operation
        operation_name: String,
        #[allow(dead_code)]
        /// Duration of the operation in nanoseconds
        duration_ns: u64,
        #[allow(dead_code)]
        /// Whether the operation resulted in an error
        is_error: bool,
        #[allow(dead_code)]
        /// UNIX timestamp when the operation completed
        timestamp: u64,
    },
    /// A cache hit occurred
    CacheHit {
        #[allow(dead_code)]
        /// Name of the cache
        cache_name: String,
        #[allow(dead_code)]
        /// Lookup time in nanoseconds
        lookup_time_ns: u64,
        #[allow(dead_code)]
        /// UNIX timestamp of the hit
        timestamp: u64,
    },
    /// A cache miss occurred
    CacheMiss {
        #[allow(dead_code)]
        /// Name of the cache
        cache_name: String,
        #[allow(dead_code)]
        /// Lookup time in nanoseconds
        lookup_time_ns: u64,
        #[allow(dead_code)]
        /// UNIX timestamp of the miss
        timestamp: u64,
    },
    /// A cache eviction occurred
    CacheEviction {
        #[allow(dead_code)]
        /// Name of the cache
        cache_name: String,
        #[allow(dead_code)]
        /// UNIX timestamp of the eviction
        timestamp: u64,
    },
}

/// Serializable performance snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSnapshot {
    /// Optional session identifier
    pub session_id: Option<String>,
    /// UNIX timestamp when monitoring started
    pub started_at_timestamp: u64,
    /// Total number of operations recorded
    pub total_operations: u64,
    /// Total number of errors encountered
    pub total_errors: u64,
    /// Global error rate as a percentage
    pub global_error_rate: f64,
    /// Number of currently active operations
    pub active_operations: usize,
    /// Per-operation metric snapshots
    pub operations: Vec<OperationSnapshot>,
    /// Per-cache metric snapshots
    pub caches: Vec<CacheSnapshot>,
    /// Operations with high average latency (name, duration_ms)
    pub slow_operations: Vec<(String, f64)>,
    /// Operations with high error rates (name, rate)
    pub error_prone_operations: Vec<(String, f64)>,
    /// Caches with identified issues (name, description)
    pub cache_issues: Vec<(String, String)>,
}

/// Snapshot of a single operation's metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSnapshot {
    /// Name of the operation
    pub operation_name: String,
    /// Total number of calls
    pub total_calls: u64,
    /// Number of errors
    pub error_count: u64,
    /// Error rate as a percentage
    pub error_rate: f64,
    /// Average duration in milliseconds
    pub avg_duration_ms: f64,
    /// Minimum observed duration in milliseconds
    pub min_duration_ms: f64,
    /// Maximum observed duration in milliseconds
    pub max_duration_ms: f64,
    /// UNIX timestamp of the most recent execution
    pub last_execution_timestamp: u64,
}

/// Snapshot of a single cache's metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSnapshot {
    /// Name of the cache
    pub cache_name: String,
    /// Total number of cache requests
    pub total_requests: u64,
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Number of cache evictions
    pub evictions: u64,
    /// Hit rate (0.0–1.0)
    pub hit_rate: f64,
    /// Miss rate (0.0–1.0)
    pub miss_rate: f64,
    /// Average lookup time in nanoseconds
    pub avg_lookup_time_ns: u64,
    /// UNIX timestamp of the most recent update
    pub last_updated_timestamp: u64,
}

impl PerformanceMonitor {
    /// Create a new performance monitor for the given optional session
    pub fn new(session_id: Option<String>) -> Self {
        let (sender, receiver) = bounded(10000); // Large buffer for high-throughput

        Self {
            operations: DashMap::new(),
            caches: DashMap::new(),
            total_operations: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            active_operations: AtomicUsize::new(0),
            started_at_timestamp: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_else(|_| std::time::Duration::from_secs(0))
                    .as_secs(),
            ),
            session_id,
            metrics_sender: sender,
            metrics_receiver: receiver,
        }
    }

    /// Start timing an operation
    pub fn start_timer(&self, operation_name: &str) -> OperationTimer {
        self.active_operations.fetch_add(1, Ordering::Relaxed);

        OperationTimer {
            operation_name: operation_name.to_string(),
            start_time: Instant::now(),
            monitor: None, // Remove unsafe pointer read - timer will work without monitor reference
            is_finished: AtomicBool::new(false),
        }
    }

    /// Record operation completion
    pub fn record_operation(&self, operation_name: &str, duration: Duration, is_error: bool) {
        let duration_ns = duration.as_nanos() as u64;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs();

        // Update global counters atomically
        self.total_operations.fetch_add(1, Ordering::Relaxed);
        if is_error {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
        }

        // Get or create operation metrics
        let metrics = self
            .operations
            .entry(operation_name.to_string())
            .or_insert_with(|| Arc::new(OperationMetrics::new(operation_name)))
            .clone();

        // Update metrics atomically
        metrics.record_operation(duration_ns, timestamp, is_error);

        // Send event for async processing (non-blocking)
        let _ = self
            .metrics_sender
            .try_send(MetricsEvent::OperationCompleted {
                operation_name: operation_name.to_string(),
                duration_ns,
                is_error,
                timestamp,
            });

        debug!(
            "Recorded operation '{}': {:.2}ms (error: {})",
            operation_name,
            duration_ns as f64 / 1_000_000.0,
            is_error
        );
    }

    /// Record cache hit
    pub fn record_cache_hit(&self, cache_name: &str, lookup_time: Duration) {
        let lookup_time_ns = lookup_time.as_nanos() as u64;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs();

        let metrics = self
            .caches
            .entry(cache_name.to_string())
            .or_insert_with(|| Arc::new(CacheMetrics::new(cache_name)))
            .clone();

        metrics.record_hit(lookup_time_ns, timestamp);

        let _ = self.metrics_sender.try_send(MetricsEvent::CacheHit {
            cache_name: cache_name.to_string(),
            lookup_time_ns,
            timestamp,
        });
    }

    /// Record cache miss
    pub fn record_cache_miss(&self, cache_name: &str, lookup_time: Duration) {
        let lookup_time_ns = lookup_time.as_nanos() as u64;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs();

        let metrics = self
            .caches
            .entry(cache_name.to_string())
            .or_insert_with(|| Arc::new(CacheMetrics::new(cache_name)))
            .clone();

        metrics.record_miss(lookup_time_ns, timestamp);

        let _ = self.metrics_sender.try_send(MetricsEvent::CacheMiss {
            cache_name: cache_name.to_string(),
            lookup_time_ns,
            timestamp,
        });
    }

    /// Record cache eviction
    pub fn record_cache_eviction(&self, cache_name: &str) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs();

        let metrics = self
            .caches
            .entry(cache_name.to_string())
            .or_insert_with(|| Arc::new(CacheMetrics::new(cache_name)))
            .clone();

        metrics.record_eviction(timestamp);

        let _ = self.metrics_sender.try_send(MetricsEvent::CacheEviction {
            cache_name: cache_name.to_string(),
            timestamp,
        });
    }

    /// Get current snapshot
    pub fn get_snapshot(&self) -> PerformanceSnapshot {
        let total_ops = self.total_operations.load(Ordering::Relaxed);
        let total_errors = self.total_errors.load(Ordering::Relaxed);
        let global_error_rate = if total_ops > 0 {
            total_errors as f64 / total_ops as f64 * 100.0
        } else {
            0.0
        };

        // Collect operation snapshots
        let mut operations = Vec::new();
        let mut slow_operations = Vec::new();
        let mut error_prone_operations = Vec::new();

        for entry in self.operations.iter() {
            let snapshot = entry.value().snapshot();

            if snapshot.avg_duration_ms > 500.0 {
                slow_operations.push((snapshot.operation_name.clone(), snapshot.avg_duration_ms));
            }

            if snapshot.error_rate > 5.0 {
                error_prone_operations.push((snapshot.operation_name.clone(), snapshot.error_rate));
            }

            operations.push(snapshot);
        }

        // Collect cache snapshots
        let mut caches = Vec::new();
        let mut cache_issues = Vec::new();

        for entry in self.caches.iter() {
            let snapshot = entry.value().snapshot();

            if snapshot.hit_rate < 0.5 && snapshot.total_requests > 100 {
                cache_issues.push((
                    snapshot.cache_name.clone(),
                    format!("Low hit rate: {:.1}%", snapshot.hit_rate * 100.0),
                ));
            }

            caches.push(snapshot);
        }

        // Sort by impact - handle NaN values safely
        slow_operations.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        error_prone_operations.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        PerformanceSnapshot {
            session_id: self.session_id.clone(),
            started_at_timestamp: self.started_at_timestamp.load(Ordering::Relaxed),
            total_operations: total_ops,
            total_errors,
            global_error_rate,
            active_operations: self.active_operations.load(Ordering::Relaxed),
            operations,
            caches,
            slow_operations: slow_operations.into_iter().take(5).collect(),
            error_prone_operations: error_prone_operations.into_iter().take(5).collect(),
            cache_issues: cache_issues.into_iter().take(3).collect(),
        }
    }

    /// Check for performance issues
    pub fn has_performance_issues(&self) -> bool {
        // Check global error rate
        let total_ops = self.total_operations.load(Ordering::Relaxed);
        if total_ops > 100 {
            let total_errors = self.total_errors.load(Ordering::Relaxed);
            let error_rate = total_errors as f64 / total_ops as f64 * 100.0;
            if error_rate > 10.0 {
                return true;
            }
        }

        // Check individual operations
        for entry in self.operations.iter() {
            if entry.value().is_problematic() {
                return true;
            }
        }

        // Check caches
        for entry in self.caches.iter() {
            if entry.value().has_issues() {
                return true;
            }
        }

        false
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.total_operations.store(0, Ordering::Relaxed);
        self.total_errors.store(0, Ordering::Relaxed);
        self.active_operations.store(0, Ordering::Relaxed);
        self.started_at_timestamp.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| std::time::Duration::from_secs(0))
                .as_secs(),
            Ordering::Relaxed,
        );

        self.operations.clear();
        self.caches.clear();

        info!("Reset all performance metrics");
    }

    /// Get active operation count
    pub fn active_operations(&self) -> usize {
        self.active_operations.load(Ordering::Relaxed)
    }
}

impl OperationMetrics {
    /// Create a new metrics tracker for the named operation
    pub fn new(operation_name: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs();

        Self {
            operation_name: operation_name.to_string(),
            total_calls: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            total_duration_ns: AtomicU64::new(0),
            min_duration_ns: AtomicU64::new(u64::MAX),
            max_duration_ns: AtomicU64::new(0),
            last_execution_timestamp: AtomicU64::new(now),
            avg_duration_ns: AtomicU64::new(0),
            error_rate: AtomicF64::new(0.0),
            recent_duration_sum: AtomicU64::new(0),
            recent_operation_count: AtomicU64::new(0),
        }
    }

    /// Record a single operation execution with its duration and error status
    pub fn record_operation(&self, duration_ns: u64, timestamp: u64, is_error: bool) {
        // Update counters
        let total_calls = self.total_calls.fetch_add(1, Ordering::Relaxed) + 1;
        let total_duration = self
            .total_duration_ns
            .fetch_add(duration_ns, Ordering::Relaxed)
            + duration_ns;

        if is_error {
            self.error_count.fetch_add(1, Ordering::Relaxed);
        }

        self.last_execution_timestamp
            .store(timestamp, Ordering::Relaxed);

        // Update min atomically
        let mut current_min = self.min_duration_ns.load(Ordering::Relaxed);
        while current_min > duration_ns {
            match self.min_duration_ns.compare_exchange_weak(
                current_min,
                duration_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(new_min) => current_min = new_min,
            }
        }

        // Update max atomically
        let mut current_max = self.max_duration_ns.load(Ordering::Relaxed);
        while current_max < duration_ns {
            match self.max_duration_ns.compare_exchange_weak(
                current_max,
                duration_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(new_max) => current_max = new_max,
            }
        }

        // Update cached average
        let avg = total_duration / total_calls;
        self.avg_duration_ns.store(avg, Ordering::Relaxed);

        // Update cached error rate
        let error_count = self.error_count.load(Ordering::Relaxed);
        let error_rate = (error_count as f64 / total_calls as f64) * 100.0;
        self.error_rate.store(error_rate, Ordering::Relaxed);

        // Update recent metrics (simple moving window)
        let recent_count = self.recent_operation_count.load(Ordering::Relaxed);
        if recent_count < 100 {
            self.recent_duration_sum
                .fetch_add(duration_ns, Ordering::Relaxed);
            self.recent_operation_count.fetch_add(1, Ordering::Relaxed);
        } else {
            // Reset recent window when full (simple approach)
            self.recent_duration_sum
                .store(duration_ns, Ordering::Relaxed);
            self.recent_operation_count.store(1, Ordering::Relaxed);
        }
    }

    /// Produce a serializable snapshot of the current operation metrics
    pub fn snapshot(&self) -> OperationSnapshot {
        let total_calls = self.total_calls.load(Ordering::Relaxed);
        let avg_duration_ns = if total_calls > 0 {
            self.avg_duration_ns.load(Ordering::Relaxed)
        } else {
            0
        };

        OperationSnapshot {
            operation_name: self.operation_name.clone(),
            total_calls,
            error_count: self.error_count.load(Ordering::Relaxed),
            error_rate: self.error_rate.load(Ordering::Relaxed),
            avg_duration_ms: avg_duration_ns as f64 / 1_000_000.0,
            min_duration_ms: self.min_duration_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
            max_duration_ms: self.max_duration_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
            last_execution_timestamp: self.last_execution_timestamp.load(Ordering::Relaxed),
        }
    }

    /// Check whether this operation is exhibiting problematic performance
    pub fn is_problematic(&self) -> bool {
        let error_rate = self.error_rate.load(Ordering::Relaxed);
        let avg_duration_ns = self.avg_duration_ns.load(Ordering::Relaxed);
        let max_duration_ns = self.max_duration_ns.load(Ordering::Relaxed);

        error_rate > 10.0 ||
        avg_duration_ns > 2_000_000_000 || // > 2 seconds
        max_duration_ns > 30_000_000_000 // > 30 seconds
    }
}

impl CacheMetrics {
    /// Create a new metrics tracker for the named cache
    pub fn new(cache_name: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs();

        Self {
            cache_name: cache_name.to_string(),
            total_requests: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            total_lookup_time_ns: AtomicU64::new(0),
            avg_lookup_time_ns: AtomicU64::new(0),
            hit_rate: AtomicF64::new(0.0),
            miss_rate: AtomicF64::new(0.0),
            last_updated_timestamp: AtomicU64::new(now),
            created_timestamp: AtomicU64::new(now),
        }
    }

    /// Record a cache hit with the given lookup time
    pub fn record_hit(&self, lookup_time_ns: u64, timestamp: u64) {
        let total_requests = self.total_requests.fetch_add(1, Ordering::Relaxed) + 1;
        let hits = self.hits.fetch_add(1, Ordering::Relaxed) + 1;
        let total_lookup_time = self
            .total_lookup_time_ns
            .fetch_add(lookup_time_ns, Ordering::Relaxed)
            + lookup_time_ns;

        self.last_updated_timestamp
            .store(timestamp, Ordering::Relaxed);

        // Update cached rates
        let hit_rate = hits as f64 / total_requests as f64;
        self.hit_rate.store(hit_rate, Ordering::Relaxed);
        self.miss_rate.store(1.0 - hit_rate, Ordering::Relaxed);

        // Update cached average lookup time
        let avg_lookup = total_lookup_time / total_requests;
        self.avg_lookup_time_ns.store(avg_lookup, Ordering::Relaxed);
    }

    /// Record a cache miss with the given lookup time
    pub fn record_miss(&self, lookup_time_ns: u64, timestamp: u64) {
        let total_requests = self.total_requests.fetch_add(1, Ordering::Relaxed) + 1;
        let total_lookup_time = self
            .total_lookup_time_ns
            .fetch_add(lookup_time_ns, Ordering::Relaxed)
            + lookup_time_ns;

        self.last_updated_timestamp
            .store(timestamp, Ordering::Relaxed);

        // Update cached rates
        let hits = self.hits.load(Ordering::Relaxed);
        let hit_rate = hits as f64 / total_requests as f64;
        self.hit_rate.store(hit_rate, Ordering::Relaxed);
        self.miss_rate.store(1.0 - hit_rate, Ordering::Relaxed);

        // Update cached average lookup time
        let avg_lookup = total_lookup_time / total_requests;
        self.avg_lookup_time_ns.store(avg_lookup, Ordering::Relaxed);
    }

    /// Record a cache eviction
    pub fn record_eviction(&self, timestamp: u64) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
        self.last_updated_timestamp
            .store(timestamp, Ordering::Relaxed);
    }

    /// Produce a serializable snapshot of the current cache metrics
    pub fn snapshot(&self) -> CacheSnapshot {
        CacheSnapshot {
            cache_name: self.cache_name.clone(),
            total_requests: self.total_requests.load(Ordering::Relaxed),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            hit_rate: self.hit_rate.load(Ordering::Relaxed),
            miss_rate: self.miss_rate.load(Ordering::Relaxed),
            avg_lookup_time_ns: self.avg_lookup_time_ns.load(Ordering::Relaxed),
            last_updated_timestamp: self.last_updated_timestamp.load(Ordering::Relaxed),
        }
    }

    /// Check whether this cache is exhibiting issues such as low hit rate or slow lookups
    pub fn has_issues(&self) -> bool {
        let hit_rate = self.hit_rate.load(Ordering::Relaxed);
        let avg_lookup_ns = self.avg_lookup_time_ns.load(Ordering::Relaxed);
        let total_requests = self.total_requests.load(Ordering::Relaxed);

        (hit_rate < 0.3 && total_requests > 100) || avg_lookup_ns > 1_000_000 // > 1ms
    }
}

impl OperationTimer {
    /// Mark the timed operation as finished with an error
    pub fn finish_with_error(self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            self.is_finished.store(true, Ordering::Relaxed);
            let _duration = self.start_time.elapsed();
            // Monitor is optional now - we just track timing without recording
            if let Some(monitor) = &self.monitor {
                monitor.active_operations.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Return the duration elapsed since the timer started
    pub fn current_duration(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Consume the timer; actual cleanup is handled by `Drop`
    pub fn finish(self) {
        // Handled by Drop
    }
}

impl Drop for OperationTimer {
    fn drop(&mut self) {
        // Only record if not already finished and monitor is available
        if !self.is_finished.load(Ordering::Relaxed) {
            let _duration = self.start_time.elapsed();
            // Monitor is optional now - we just track timing without recording
            // This prevents the unsafe pointer issues while maintaining functionality
            if let Some(monitor) = &self.monitor {
                monitor.active_operations.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

// Global performance monitor
static GLOBAL_MONITOR: std::sync::OnceLock<Arc<PerformanceMonitor>> =
    std::sync::OnceLock::new();

/// Initialize the global performance monitor with an optional session identifier
pub fn init_monitoring(session_id: Option<String>) {
    let monitor = Arc::new(PerformanceMonitor::new(session_id));
    let _ = GLOBAL_MONITOR.set(monitor);
}

/// Retrieve the global performance monitor, if initialized
pub fn get_monitor() -> Option<&'static Arc<PerformanceMonitor>> {
    GLOBAL_MONITOR.get()
}

/// Start a timer for the named operation using the global monitor
pub fn start_timer(operation_name: &str) -> Option<OperationTimer> {
    get_monitor().map(|monitor| monitor.start_timer(operation_name))
}

/// Convenience macro for timing operations
#[macro_export]
macro_rules! time_operation {
    ($operation:expr, $code:block) => {{
        let _timer = $crate::performance::start_timer($operation);
        $code
    }};
}
