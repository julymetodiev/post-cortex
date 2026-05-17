// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Summary-refresh work queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use uuid::Uuid;

use super::PipelineError;

/// Refresh the structured summary view for a session.
#[derive(Debug, Clone)]
pub struct SummaryWorkItem {
    /// Session whose summary should be regenerated.
    pub session_id: Uuid,
}

/// Bounded MPSC queue feeding the summary-refresh worker task.
pub struct SummaryQueue {
    sender: mpsc::Sender<SummaryWorkItem>,
    backlog: Arc<AtomicUsize>,
}

impl SummaryQueue {
    /// Spawn the summary worker on the current Tokio runtime and return the sending handle.
    #[must_use]
    pub fn start(capacity: usize, backlog: Arc<AtomicUsize>) -> Self {
        let (tx, mut rx) = mpsc::channel::<SummaryWorkItem>(capacity);
        let worker_backlog = Arc::clone(&backlog);

        tokio::spawn(async move {
            debug!("summary pipeline worker started");
            while let Some(item) = rx.recv().await {
                trace!(session_id = %item.session_id, "summary pipeline: processing item");
                // Phase 5 stub. Phase 7 wires this to call into
                // post_cortex_core::summary::SummaryGenerator after
                // re-loading the session's active state.
                worker_backlog.fetch_sub(1, Ordering::Relaxed);
            }
            warn!("summary pipeline worker exiting (channel closed)");
        });

        Self {
            sender: tx,
            backlog,
        }
    }

    /// Non-blocking submit; returns [`PipelineError::Backpressure`] if the queue is full.
    pub fn submit(&self, item: SummaryWorkItem) -> Result<(), PipelineError> {
        match self.sender.try_send(item) {
            Ok(()) => {
                self.backlog.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                Err(PipelineError::Backpressure { queue: "summary" })
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(PipelineError::WorkerShutdown { queue: "summary" })
            }
        }
    }
}
