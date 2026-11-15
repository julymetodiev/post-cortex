use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Global system metrics - all atomic
#[derive(Debug)]
pub struct SystemMetrics {
    /// Total number of requests processed
    pub total_requests: AtomicU64,
    /// Number of successful requests
    pub successful_requests: AtomicU64,
    /// Number of failed requests
    pub failed_requests: AtomicU64,
    /// Number of currently active operations
    pub active_operations: AtomicUsize,
    /// UNIX timestamp when metrics collection started
    pub start_timestamp: AtomicU64,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMetrics {
    /// Create a new `SystemMetrics` instance with all counters zeroed
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            active_operations: AtomicUsize::new(0),
            start_timestamp: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            ),
        }
    }
}
