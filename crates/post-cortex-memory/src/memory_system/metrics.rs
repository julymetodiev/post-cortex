use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Global system metrics - all atomic
#[derive(Debug)]
pub struct SystemMetrics {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub active_operations: AtomicUsize,
    pub storage_operations: AtomicU64,
    pub cache_operations: AtomicU64,
    pub start_timestamp: AtomicU64,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMetrics {
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
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub active_operations: usize,
    pub active_sessions: usize,
    pub circuit_breaker_open: bool,
    pub circuit_breaker_failures: u64,
    pub cache_hit_rate: f64,
    pub contexts_processed: u64,
    pub processing_errors: u64,
    pub storage_operations: u64,
    pub uptime_seconds: u64,
}

/// Session manager metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManagerMetrics {
    pub session_count: usize,
    pub active_sessions: usize,
    pub total_operations: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_hit_rate: f64,
    pub cache_size: usize,
    pub cache_capacity: usize,
}

/// Circuit breaker stats snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerStats {
    pub is_open: bool,
    pub failure_count: u64,
    pub success_count: u64,
    pub last_failure_timestamp: u64,
    pub last_success_timestamp: u64,
}
