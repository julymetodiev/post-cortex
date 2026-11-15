// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The record commit: one compare-and-swap of the session (the record and
//! its graph merge) and one persist, behind the circuit breaker.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::normalised;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::capabilities::records::{RecordError, Records};
use crate::memory_system::MemoryRuntime;

/// A record committed by [`Records::commit`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Committed {
    /// Id of the new entry.
    pub(crate) entry_id: Uuid,
    /// Whether the record was confirmed written to storage before the call
    /// returned (only ever true when the caller asked to wait for it).
    pub(crate) durable: bool,
}

impl Records {
    /// Add a validated record to an existing session: the record and its
    /// merge into the entity graph are one compare-and-swap and one
    /// persist. The record is normalised once and shared across retried
    /// swaps. Embedding it is derived work (see [`Records::write`]).
    ///
    /// With `wait_durable`, returns only after the record's storage write
    /// finished; [`Committed::durable`] reports whether it succeeded. A
    /// failed write does not fail the call: the record is committed in
    /// memory and later persists of the session retry the write.
    pub(crate) async fn commit(
        &self,
        session_id: Uuid,
        update: ContextUpdate,
        wait_durable: bool,
    ) -> Result<Committed, RecordError> {
        let rt = &self.rt;
        rt.system_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);
        rt.system_metrics
            .active_operations
            .fetch_add(1, Ordering::Relaxed);

        // A failing backend is refused at the adapter boundary (its
        // breaker) and a failing module at the host's.
        let result = self.swap_in(session_id, update, wait_durable).await;
        let counter = match &result {
            Ok(_) => &rt.system_metrics.successful_requests,
            Err(_) => &rt.system_metrics.failed_requests,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        rt.system_metrics
            .active_operations
            .fetch_sub(1, Ordering::Relaxed);
        result
    }

    async fn swap_in(
        &self,
        session_id: Uuid,
        update: ContextUpdate,
        wait_durable: bool,
    ) -> Result<Committed, RecordError> {
        // The session must exist: a write never creates one, so a late
        // write or a mistyped id cannot bring a session into existence.
        let handle = self.handle(session_id).await?;

        // Normalised once; every attempt shares it. The record and its
        // graph merge are one swap, so the merge is neither lost nor
        // applied twice.
        let stored = Arc::new(normalised(update));
        MemoryRuntime::cas_update_session(&handle, session_id, "add_update", |session| {
            session.apply_normalised_record(Arc::clone(&stored));
            true
        })
        .await
        .inspect_err(|e| warn!("Failed to add record: {}", e))
        .map_err(RecordError::Storage)?;
        let record = ContextUpdate::clone(&stored);
        let entry_id = record.id;

        // Committed in memory by the swap; the persister coalesces this with
        // other pending writes.
        let persister = &self.rt.persister;
        let durable = if wait_durable {
            match persister.persist_and_wait(&handle, vec![record]).await {
                Ok(()) => true,
                Err(e) => {
                    warn!("Update {entry_id} of session {session_id} not yet persisted: {e}");
                    false
                }
            }
        } else {
            persister.persist_nowait(&handle, vec![record]);
            false
        };

        debug!("Added record {} to session {}", entry_id, session_id);
        Ok(Committed { entry_id, durable })
    }
}
