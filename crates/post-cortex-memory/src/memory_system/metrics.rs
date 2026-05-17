use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

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
    /// Number of storage-layer operations
    pub storage_operations: AtomicU64,
    /// Number of cache-layer operations
    pub cache_operations: AtomicU64,
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
            storage_operations: AtomicU64::new(0),
            cache_operations: AtomicU64::new(0),
            start_timestamp: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            ),
        }
    }
}

/// System health snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    /// Total number of requests processed
    pub total_requests: u64,
    /// Number of successful requests
    pub successful_requests: u64,
    /// Number of failed requests
    pub failed_requests: u64,
    /// Number of currently active operations
    pub active_operations: usize,
    /// Number of currently active sessions
    pub active_sessions: usize,
    /// Whether the circuit breaker is currently open
    pub circuit_breaker_open: bool,
    /// Number of failures recorded by the circuit breaker
    pub circuit_breaker_failures: u64,
    /// Overall cache hit rate (0.0–1.0)
    pub cache_hit_rate: f64,
    /// Number of context updates processed
    pub contexts_processed: u64,
    /// Number of processing errors encountered
    pub processing_errors: u64,
    /// Number of storage-layer operations
    pub storage_operations: u64,
    /// System uptime in seconds
    pub uptime_seconds: u64,
}

/// Session manager metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManagerMetrics {
    /// Total number of sessions managed
    pub session_count: usize,
    /// Number of currently active sessions
    pub active_sessions: usize,
    /// Total number of session operations
    pub total_operations: u64,
    /// Number of session cache hits
    pub cache_hits: u64,
    /// Number of session cache misses
    pub cache_misses: u64,
    /// Session cache hit rate (0.0–1.0)
    pub cache_hit_rate: f64,
    /// Current number of entries in the session cache
    pub cache_size: usize,
    /// Maximum capacity of the session cache
    pub cache_capacity: usize,
}

/// Circuit breaker stats snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerStats {
    /// Whether the circuit breaker is currently open
    pub is_open: bool,
    /// Number of consecutive failures recorded
    pub failure_count: u64,
    /// Number of consecutive successes recorded
    pub success_count: u64,
    /// UNIX timestamp of the most recent failure
    pub last_failure_timestamp: u64,
    /// UNIX timestamp of the most recent success
    pub last_success_timestamp: u64,
}
