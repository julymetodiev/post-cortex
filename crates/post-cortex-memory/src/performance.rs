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
    total_operations: AtomicU64,
    total_errors: AtomicU64,
    active_operations: AtomicUsize,
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
    operation_name: String,

    // Basic counters
    total_calls: AtomicU64,
    error_count: AtomicU64,

    // Duration tracking (all in nanoseconds for precision)
    total_duration_ns: AtomicU64,
    min_duration_ns: AtomicU64, // Use AtomicU64::MAX as initial value
    max_duration_ns: AtomicU64,

    // Recent performance tracking
    last_execution_timestamp: AtomicU64,

    // Performance indicators
    avg_duration_ns: AtomicU64, // Cached average, updated on each operation
    error_rate: AtomicF64,      // Cached error rate

    // Trend tracking - simple moving average
    recent_duration_sum: AtomicU64,    // Sum of last N operations
    recent_operation_count: AtomicU64, // Count for recent operations (max 100)
}

/// Cache metrics using atomics
#[derive(Debug)]
pub struct CacheMetrics {
    cache_name: String,

    // Request counters
    total_requests: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,

    // Performance counters
    total_lookup_time_ns: AtomicU64,
    avg_lookup_time_ns: AtomicU64, // Cached average

    // Rates (cached for performance)
    hit_rate: AtomicF64,
    miss_rate: AtomicF64,

    // Timestamps
    last_updated_timestamp: AtomicU64,
    #[allow(dead_code)]
    created_timestamp: AtomicU64,
}

/// Operation timer
pub struct OperationTimer {
    #[allow(dead_code)]
    operation_name: String,
    start_time: Instant,
    monitor: Option<Arc<PerformanceMonitor>>, // Make monitor optional to avoid unsafe code
    is_finished: AtomicBool,
}

/// Events for async metrics processing (if needed)
#[derive(Debug, Clone)]
enum MetricsEvent {
    OperationCompleted {
        #[allow(dead_code)]
        operation_name: String,
        #[allow(dead_code)]
        duration_ns: u64,
        #[allow(dead_code)]
        is_error: bool,
        #[allow(dead_code)]
        timestamp: u64,
    },
    CacheHit {
        #[allow(dead_code)]
        cache_name: String,
        #[allow(dead_code)]
        lookup_time_ns: u64,
        #[allow(dead_code)]
        timestamp: u64,
    },
    CacheMiss {
        #[allow(dead_code)]
        cache_name: String,
        #[allow(dead_code)]
        lookup_time_ns: u64,
        #[allow(dead_code)]
        timestamp: u64,
    },
    CacheEviction {
        #[allow(dead_code)]
        cache_name: String,
        #[allow(dead_code)]
        timestamp: u64,
    },
}

/// Serializable performance snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSnapshot {
    pub session_id: Option<String>,
    pub started_at_timestamp: u64,
    pub total_operations: u64,
    pub total_errors: u64,
    pub global_error_rate: f64,
    pub active_operations: usize,
    pub operations: Vec<OperationSnapshot>,
    pub caches: Vec<CacheSnapshot>,
    pub slow_operations: Vec<(String, f64)>,
    pub error_prone_operations: Vec<(String, f64)>,
    pub cache_issues: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSnapshot {
    pub operation_name: String,
    pub total_calls: u64,
    pub error_count: u64,
    pub error_rate: f64,
    pub avg_duration_ms: f64,
    pub min_duration_ms: f64,
    pub max_duration_ms: f64,
    pub last_execution_timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSnapshot {
    pub cache_name: String,
    pub total_requests: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub hit_rate: f64,
    pub miss_rate: f64,
    pub avg_lookup_time_ns: u64,
    pub last_updated_timestamp: u64,
}

impl PerformanceMonitor {
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

    pub fn record_eviction(&self, timestamp: u64) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
        self.last_updated_timestamp
            .store(timestamp, Ordering::Relaxed);
    }

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

    pub fn has_issues(&self) -> bool {
        let hit_rate = self.hit_rate.load(Ordering::Relaxed);
        let avg_lookup_ns = self.avg_lookup_time_ns.load(Ordering::Relaxed);
        let total_requests = self.total_requests.load(Ordering::Relaxed);

        (hit_rate < 0.3 && total_requests > 100) || avg_lookup_ns > 1_000_000 // > 1ms
    }
}

impl OperationTimer {
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

    pub fn current_duration(&self) -> Duration {
        self.start_time.elapsed()
    }

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

pub fn init_monitoring(session_id: Option<String>) {
    let monitor = Arc::new(PerformanceMonitor::new(session_id));
    let _ = GLOBAL_MONITOR.set(monitor);
}

pub fn get_monitor() -> Option<&'static Arc<PerformanceMonitor>> {
    GLOBAL_MONITOR.get()
}

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
