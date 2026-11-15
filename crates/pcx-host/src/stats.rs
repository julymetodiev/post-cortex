// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Per-module health (T14): calls, failures, timeouts, panics and latency
//! percentiles from a fixed bucket histogram.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Upper bounds of the latency buckets, in milliseconds; a last bucket
/// takes everything slower.
const BUCKETS_MS: [u64; 13] = [
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000,
];

/// How a guarded call ended.
#[derive(Debug, Clone, Copy)]
pub enum Ended {
    /// It returned a value.
    Done,
    /// It returned an error.
    Failed,
    /// It ran past its deadline.
    TimedOut,
    /// It panicked.
    Panicked,
    /// The caller cancelled it (not the module's failure).
    Cancelled,
    /// The module's breaker refused it.
    Refused,
}

/// The health of one module, as `Host::health` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHealth {
    /// The module.
    pub module: String,
    /// Calls and events it was given.
    pub calls: u64,
    /// Calls that returned an error.
    pub errors: u64,
    /// Calls that ran past their deadline.
    pub timeouts: u64,
    /// Calls that panicked.
    pub panics: u64,
    /// Calls the caller cancelled.
    pub cancelled: u64,
    /// Calls its open breaker refused.
    pub refused: u64,
    /// Whether its breaker is open (it keeps failing).
    pub breaker_open: bool,
    /// Median latency, as the upper bound of its bucket (none before the
    /// first call).
    pub p50: Option<Duration>,
    /// 99th percentile latency, likewise; `Duration::MAX` past the last
    /// bucket.
    pub p99: Option<Duration>,
    /// What stops it from doing its work, as it reported; empty: nothing it
    /// knows of.
    pub problem: String,
}

/// The counters of one module.
#[derive(Default)]
pub struct Stats {
    calls: AtomicU64,
    errors: AtomicU64,
    timeouts: AtomicU64,
    panics: AtomicU64,
    cancelled: AtomicU64,
    refused: AtomicU64,
    latency: [AtomicU64; BUCKETS_MS.len() + 1],
}

impl Stats {
    /// Count one call that took `elapsed` and ended as `ended`.
    pub fn record(&self, elapsed: Duration, ended: Ended) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let counter = match ended {
            Ended::Done => None,
            Ended::Failed => Some(&self.errors),
            Ended::TimedOut => Some(&self.timeouts),
            Ended::Panicked => Some(&self.panics),
            Ended::Cancelled => Some(&self.cancelled),
            Ended::Refused => Some(&self.refused),
        };
        if let Some(counter) = counter {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let bucket = BUCKETS_MS.partition_point(|bound| *bound < ms);
        self.latency[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// A snapshot for `module`, whose breaker is open or not.
    pub fn health(&self, module: &str, breaker_open: bool) -> ModuleHealth {
        let counts: Vec<u64> = self
            .latency
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .collect();
        ModuleHealth {
            module: module.to_string(),
            calls: self.calls.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            panics: self.panics.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            breaker_open,
            p50: percentile(&counts, 50),
            p99: percentile(&counts, 99),
            problem: String::new(),
        }
    }
}

/// The upper bound of the bucket holding the `pct`th percentile of `counts`.
fn percentile(counts: &[u64], pct: u64) -> Option<Duration> {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return None;
    }
    // The rank of the percentile, at least the first call.
    let rank = (total * pct).div_ceil(100).max(1);
    let mut seen = 0;
    for (bucket, count) in counts.iter().enumerate() {
        seen += count;
        if seen >= rank {
            return Some(
                BUCKETS_MS
                    .get(bucket)
                    .map_or(Duration::MAX, |ms| Duration::from_millis(*ms)),
            );
        }
    }
    Some(Duration::MAX)
}
