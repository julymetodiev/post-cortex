use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{debug, warn};

use super::metrics::CircuitBreakerStats;

/// Circuit breaker using atomics
#[derive(Debug)]
pub struct CircuitBreaker {
    /// Whether the circuit breaker is currently open (blocking requests)
    pub is_open: AtomicBool,
    /// Consecutive failure count
    pub failure_count: AtomicU64,
    /// UNIX timestamp of the most recent failure
    pub last_failure_timestamp: AtomicU64,
    /// Consecutive success count
    pub success_count: AtomicU64,
    /// UNIX timestamp of the most recent success
    pub last_success_timestamp: AtomicU64,

    /// Number of failures required to open the circuit
    pub failure_threshold: u64,
    /// Seconds to wait before allowing retries after opening
    pub timeout_seconds: u64,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given failure threshold and timeout
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

    /// Check whether the circuit breaker is currently open, resetting after the timeout period
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

    /// Record a failure, potentially opening the circuit breaker
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

    /// Record a success, gradually reducing the failure count
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

    /// Return a snapshot of the current circuit breaker statistics
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
