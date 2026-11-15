// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The one compare-and-swap loop of session changes, shared by every
//! capability that changes a session.

use crate::memory_system::{MemoryRuntime, RuntimeError};
use arc_swap::ArcSwap;
use post_cortex_core::session::active_session::ActiveSession;
use std::sync::Arc;
use uuid::Uuid;

/// Compare-and-swap attempts before a session write gives up.
const MAX_CAS_ATTEMPTS: u32 = 20;

/// Wait before retrying a lost compare-and-swap: yield for the first
/// attempts, then back off exponentially (capped at 3.2 ms).
async fn cas_backoff(attempt: u32) {
    if attempt > 3 {
        tokio::time::sleep(std::time::Duration::from_micros(
            100 * (1 << (attempt - 3).min(5)),
        ))
        .await;
    } else {
        tokio::task::yield_now().await;
    }
}

impl MemoryRuntime {
    /// Apply `mutate` to a copy of the current session and compare-and-swap
    /// it in, retrying with backoff while other writers win. `mutate`
    /// returns false when there is nothing to change; that ends the loop
    /// with `Ok(None)`. On success returns the installed snapshot.
    pub(crate) async fn cas_update_session(
        session_arc: &ArcSwap<ActiveSession>,
        session_id: Uuid,
        op: &'static str,
        mut mutate: impl FnMut(&mut ActiveSession) -> bool,
    ) -> Result<Option<Arc<ActiveSession>>, RuntimeError> {
        for attempt in 1..=MAX_CAS_ATTEMPTS {
            let current = session_arc.load_full();
            let mut new_session = (*current).clone();
            if !mutate(&mut new_session) {
                return Ok(None);
            }

            let new_arc = Arc::new(new_session);
            let prev = session_arc.compare_and_swap(&current, Arc::clone(&new_arc));
            if Arc::ptr_eq(&prev, &current) {
                return Ok(Some(new_arc));
            }
            cas_backoff(attempt).await;
        }

        Err(RuntimeError::Contention { op, session_id })
    }
}
