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

//! Atomic concurrency controller for guarding the BERT inference path against overload.

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::sleep;

/// Concurrent operation controller using atomics.
#[derive(Debug)]
pub(super) struct ConcurrencyController {
    /// Current number of active operations (atomic).
    current_operations: AtomicUsize,
    /// Maximum allowed operations.
    max_operations: usize,
    /// Flag indicating whether the controller is active.
    active: AtomicBool,
}

impl ConcurrencyController {
    pub(super) fn new(max_operations: usize) -> Self {
        Self {
            current_operations: AtomicUsize::new(0),
            max_operations,
            active: AtomicBool::new(true),
        }
    }

    /// Try to acquire a slot without blocking (CAS retry loop).
    pub(super) fn try_acquire(self: &Arc<Self>) -> Option<OperationPermit> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }

        // CAS loop to atomically acquire a slot
        loop {
            let current = self.current_operations.load(Ordering::Acquire);
            if current >= self.max_operations {
                return None;
            }

            match self.current_operations.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(OperationPermit {
                        controller: Arc::clone(self),
                    });
                }
                Err(_) => {
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Acquire with waiting (async busy wait).
    pub(super) async fn acquire(self: &Arc<Self>) -> Result<OperationPermit> {
        const MAX_WAIT_ITERATIONS: usize = 1000;
        const WAIT_DELAY_MS: u64 = 1;

        if !self.active.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("Concurrency controller is not active"));
        }

        let mut iterations = 0;
        loop {
            if let Some(permit) = self.try_acquire() {
                return Ok(permit);
            }

            iterations += 1;
            if iterations >= MAX_WAIT_ITERATIONS {
                return Err(anyhow::anyhow!(
                    "Timeout waiting for operation slot after {} iterations",
                    MAX_WAIT_ITERATIONS
                ));
            }

            sleep(Duration::from_millis(WAIT_DELAY_MS)).await;
        }
    }

    /// Release a slot (automatically called by `OperationPermit::drop`).
    fn release(&self) {
        self.current_operations.fetch_sub(1, Ordering::SeqCst);
    }

    pub(super) fn current_load(&self) -> usize {
        self.current_operations.load(Ordering::Relaxed)
    }

    pub(super) fn max_capacity(&self) -> usize {
        self.max_operations
    }
}

/// Operation permit (auto-releasing on drop).
pub(super) struct OperationPermit {
    controller: Arc<ConcurrencyController>,
}

impl Drop for OperationPermit {
    fn drop(&mut self) {
        self.controller.release();
    }
}
