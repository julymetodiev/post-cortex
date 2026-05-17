use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{debug, warn};

use super::metrics::CircuitBreakerStats;

/// Circuit breaker using atomics
#[derive(Debug)]
pub struct CircuitBreaker {
    pub is_open: AtomicBool,
    pub failure_count: AtomicU64,
    pub last_failure_timestamp: AtomicU64,
    pub success_count: AtomicU64,
    pub last_success_timestamp: AtomicU64,

    pub failure_threshold: u64,
    pub timeout_seconds: u64,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u64, timeout_seconds: u64) -> Self {
        Self {
            is_open: AtomicBool::new(false),
            failure_count: AtomicU64::new(0),
            last_failure_timestamp: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            last_success_timestamp: AtomicU64::new(0),
            failure_threshold,
            timeout_seconds,
        }
    }

    pub fn is_open(&self) -> bool {
        if !self.is_open.load(Ordering::Relaxed) {
            return false;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let last_failure = self.last_failure_timestamp.load(Ordering::Relaxed);

        if now.saturating_sub(last_failure) > self.timeout_seconds {
            self.is_open.store(false, Ordering::Relaxed);
            self.failure_count.store(0, Ordering::Relaxed);
            debug!("Circuit breaker reset after timeout");
            false
        } else {
            true
        }
    }

    pub fn record_failure(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let failures = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        self.last_failure_timestamp.store(now, Ordering::Relaxed);

        if failures >= self.failure_threshold {
            self.is_open.store(true, Ordering::Relaxed);
            warn!("Circuit breaker opened after {} failures", failures);
        }
    }

    pub fn record_success(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.success_count.fetch_add(1, Ordering::Relaxed);
        self.last_success_timestamp.store(now, Ordering::Relaxed);

        // Gradually reduce failure count on success
        let current_failures = self.failure_count.load(Ordering::Relaxed);
        if current_failures > 0 {
            self.failure_count.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub fn get_stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            is_open: self.is_open.load(Ordering::Relaxed),
            failure_count: self.failure_count.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
            last_failure_timestamp: self.last_failure_timestamp.load(Ordering::Relaxed),
            last_success_timestamp: self.last_success_timestamp.load(Ordering::Relaxed),
        }
    }
}
