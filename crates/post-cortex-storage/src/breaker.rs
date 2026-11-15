// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The breaker at the adapter boundary (ARCHITECTURE section 5): after
//! [`FAILURES`] backend failures in a row, every port call of the adapter
//! fails at once with `Unavailable` for [`OPEN_FOR`]; then calls reach the
//! backend again, and one more failure opens it again. Only the backend's
//! own failures count (the database, the file system, a blocking task);
//! a typed answer (not found, conflict, corrupt row) is not a failing
//! backend.

use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use post_cortex_ports::PortError;

use crate::port_error::{AdapterError, PortResult, Result, port};

/// Backend failures in a row that open the breaker.
const FAILURES: u32 = 5;
/// How long an open breaker refuses calls.
const OPEN_FOR: Duration = Duration::from_secs(30);

/// One adapter's breaker, shared by its clones.
#[derive(Debug, Default)]
pub struct Breaker {
    failures: AtomicU32,
    opened: Mutex<Option<Instant>>,
}

impl Breaker {
    /// Run an adapter body unless the breaker is open; count its outcome.
    ///
    /// # Errors
    /// `Unavailable` while the breaker is open; the body's error as a port
    /// error.
    pub async fn guard<T>(&self, body: impl Future<Output = Result<T>>) -> PortResult<T> {
        if let Some(left) = self.open_for() {
            return Err(PortError::Unavailable(format!(
                "the storage backend failed {FAILURES} times in a row; retried in {}s",
                left.as_secs().max(1)
            )));
        }
        let result = body.await;
        match &result {
            Ok(_) => self.failures.store(0, Ordering::Relaxed),
            Err(e) if backend(e) => self.failed(),
            Err(_) => {}
        }
        port(result)
    }

    /// How long the breaker stays open, if it is.
    fn open_for(&self) -> Option<Duration> {
        let opened = (*self.opened.lock())?;
        OPEN_FOR.checked_sub(opened.elapsed())
    }

    fn failed(&self) {
        let failures = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= FAILURES {
            *self.opened.lock() = Some(Instant::now());
            tracing::warn!(
                failures,
                "storage backend failing; breaker open for {OPEN_FOR:?}"
            );
        }
    }
}

/// Whether `e` is the backend failing rather than a typed answer or a bad
/// value.
fn backend(e: &AdapterError) -> bool {
    match e {
        AdapterError::RocksDb(_) | AdapterError::Io(_) | AdapterError::Join(_) => true,
        #[cfg(feature = "surrealdb-storage")]
        AdapterError::SurrealDb(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
