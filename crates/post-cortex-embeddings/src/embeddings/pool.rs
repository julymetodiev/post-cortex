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

//! Concurrent memory pool for reusing `Vec<f32>` allocations across embedding calls.

use crossbeam_queue::SegQueue;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::debug;

/// Memory pool using crossbeam's atomic data structures.
#[derive(Debug)]
pub(super) struct MemoryPool {
    /// Available vectors — using crossbeam's concurrent queue.
    available: SegQueue<Vec<f32>>,
    /// Maximum pool size (used in `return_vector` to limit pool growth).
    max_size: usize,
    /// Current size (atomic).
    current_size: AtomicUsize,
    /// Vector capacity for new allocations.
    vector_capacity: usize,
    /// Pool hit counter (successful pool retrievals).
    pool_hits: AtomicUsize,
    /// Pool miss counter (fallback allocations).
    pool_misses: AtomicUsize,
}

impl MemoryPool {
    pub(super) fn new(size: usize, vector_capacity: usize) -> Self {
        let pool = Self {
            available: SegQueue::new(),
            max_size: size,
            current_size: AtomicUsize::new(0),
            vector_capacity,
            pool_hits: AtomicUsize::new(0),
            pool_misses: AtomicUsize::new(0),
        };

        // Pre-populate with empty vectors
        for _ in 0..size {
            pool.available.push(Vec::with_capacity(vector_capacity));
        }
        pool.current_size.store(size, Ordering::Release);

        pool
    }

    /// Get a vector from the pool, or allocate a new one if the pool is empty.
    pub(super) fn get_or_allocate(&self) -> Vec<f32> {
        match self.available.pop() {
            Some(vec) => {
                self.pool_hits.fetch_add(1, Ordering::Relaxed);
                vec
            }
            None => {
                let misses = self.pool_misses.fetch_add(1, Ordering::Relaxed) + 1;
                if misses % 100 == 0 {
                    debug!(
                        "Memory pool exhausted: {} misses (pool_size={}, capacity={})",
                        misses, self.max_size, self.vector_capacity
                    );
                }
                Vec::with_capacity(self.vector_capacity)
            }
        }
    }

    /// Return a vector to the pool for reuse. Drops if the pool is already at max size.
    #[allow(dead_code)] // Used by tests; reserved for future BERT path reuse.
    pub(super) fn return_vector(&self, mut vec: Vec<f32>) {
        if self.available.len() < self.max_size {
            vec.clear();
            self.available.push(vec);
        }
    }

    #[allow(dead_code)]
    pub(super) fn get_stats(&self) -> PoolStats {
        PoolStats {
            available: self.available.len(),
            total: self.current_size.load(Ordering::Acquire),
            hits: self.pool_hits.load(Ordering::Relaxed),
            misses: self.pool_misses.load(Ordering::Relaxed),
        }
    }

    /// Hit rate as a percentage (0.0 – 100.0). 100% when no requests have been made.
    #[allow(dead_code)]
    pub(super) fn hit_rate(&self) -> f64 {
        let hits = self.pool_hits.load(Ordering::Relaxed) as f64;
        let misses = self.pool_misses.load(Ordering::Relaxed) as f64;
        let total = hits + misses;
        if total > 0.0 {
            (hits / total) * 100.0
        } else {
            100.0
        }
    }
}

/// Pool statistics for monitoring and debugging.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(super) struct PoolStats {
    /// Number of vectors currently available in the pool.
    pub(super) available: usize,
    /// Total pool capacity.
    pub(super) total: usize,
    /// Number of successful pool retrievals.
    pub(super) hits: usize,
    /// Number of fallback allocations (pool exhaustion events).
    pub(super) misses: usize,
}
