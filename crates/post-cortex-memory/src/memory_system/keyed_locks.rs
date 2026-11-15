// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Async mutexes by name (per project key, workspace name, workspace id,
//! membership), shared by the workspace and project capabilities.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Async mutexes by name, dropped once nobody holds or waits for them.
#[derive(Default)]
pub struct KeyedLocks(DashMap<String, Arc<Mutex<()>>>);

impl KeyedLocks {
    /// Whether no lock is held or waited for.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) async fn lock(&self, name: String) -> KeyedGuard<'_> {
        let mutex = self.0.entry(name.clone()).or_default().clone();
        let guard = mutex.lock_owned().await;
        KeyedGuard {
            locks: self,
            name,
            guard: Some(guard),
        }
    }
}

/// Holds one lock of [`KeyedLocks`].
pub struct KeyedGuard<'a> {
    locks: &'a KeyedLocks,
    name: String,
    guard: Option<OwnedMutexGuard<()>>,
}

impl Drop for KeyedGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        // Waiters hold a clone; only the map's reference left means none.
        self.locks
            .0
            .remove_if(&self.name, |_, mutex| Arc::strong_count(mutex) == 1);
    }
}
