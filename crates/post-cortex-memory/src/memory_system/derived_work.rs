// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Count of derived work (embeddings, graph merges) queued or running per
//! session, so a shutdown can wait for it and an eviction can wait it out.
//!
//! Every job takes a [`DerivedWorkGuard`] when it is queued and drops it
//! when it is done, after it queued the session persist that stores its
//! result. [`MemoryRuntime::flush_all`] waits for the count to
//! reach zero before it flushes the persister, so the vectors and the
//! session state the jobs produced are written before the process stops.
//!
//! [`MemoryRuntime::flush_all`]: crate::memory_system::MemoryRuntime::flush_all

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Notify;
use uuid::Uuid;

/// Derived work in flight. See the module docs.
#[derive(Default)]
pub(crate) struct DerivedWork {
    total: AtomicUsize,
    per_session: DashMap<Uuid, usize>,
    idle: Notify,
}

/// One queued or running job; dropping it ends the job.
pub(crate) struct DerivedWorkGuard {
    work: Arc<DerivedWork>,
    session_id: Uuid,
}

impl DerivedWork {
    /// Start a job for `session_id`.
    pub(crate) fn begin(self: &Arc<Self>, session_id: Uuid) -> DerivedWorkGuard {
        *self.per_session.entry(session_id).or_default() += 1;
        self.total.fetch_add(1, Ordering::SeqCst);
        DerivedWorkGuard {
            work: Arc::clone(self),
            session_id,
        }
    }

    /// Jobs queued or running.
    pub(crate) fn pending(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }

    /// Whether `session_id` has a job queued or running.
    pub(crate) fn is_busy(&self, session_id: Uuid) -> bool {
        self.per_session.contains_key(&session_id)
    }

    /// Wait until no job is queued or running, at most `limit`. Returns
    /// whether that happened.
    pub(crate) async fn wait_idle(&self, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            // Registered before the check, so a wake-up in between is kept.
            notified.as_mut().enable();
            if self.pending() == 0 {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.pending() == 0;
            }
        }
    }
}

impl Drop for DerivedWorkGuard {
    fn drop(&mut self) {
        self.work
            .per_session
            .remove_if_mut(&self.session_id, |_, n| {
                *n -= 1;
                *n == 0
            });
        if self.work.total.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.work.idle.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_idle_returns_when_the_last_job_ends() {
        let work = Arc::new(DerivedWork::default());
        assert!(work.wait_idle(Duration::from_millis(1)).await);

        let id = Uuid::new_v4();
        let a = work.begin(id);
        let b = work.begin(id);
        assert!(work.is_busy(id));
        assert!(!work.wait_idle(Duration::from_millis(20)).await);

        drop(a);
        assert!(work.is_busy(id), "one job left");
        let waiter = {
            let work = Arc::clone(&work);
            tokio::spawn(async move { work.wait_idle(Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(b);
        assert!(waiter.await.unwrap());
        assert!(!work.is_busy(id));
        assert_eq!(work.pending(), 0);
    }
}
