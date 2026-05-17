// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding work queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use uuid::Uuid;

use super::PipelineError;

/// One unit of embedding work.
#[derive(Debug, Clone)]
pub struct EmbeddingWorkItem {
    /// Session that owns the content.
    pub session_id: Uuid,
    /// Context-update entry to vectorise.
    pub entry_id: Uuid,
    /// Raw text to embed.
    pub text: String,
}

/// Handle to the bounded MPSC channel feeding the embedding worker.
pub struct EmbeddingQueue {
    sender: mpsc::Sender<EmbeddingWorkItem>,
    backlog: Arc<AtomicUsize>,
}

impl EmbeddingQueue {
    /// Spawn the embedding worker on the current Tokio runtime and
    /// return the sending handle.
    #[must_use]
    pub fn start(capacity: usize, backlog: Arc<AtomicUsize>) -> Self {
        let (tx, mut rx) = mpsc::channel::<EmbeddingWorkItem>(capacity);
        let worker_backlog = Arc::clone(&backlog);

        tokio::spawn(async move {
            debug!("embedding pipeline worker started");
            while let Some(item) = rx.recv().await {
                trace!(
                    session_id = %item.session_id,
                    entry_id = %item.entry_id,
                    bytes = item.text.len(),
                    "embedding pipeline: processing item"
                );
                // Phase 5 stub — Phase 7 wires this to call into
                // post_cortex_memory::content_vectorizer with the
                // appropriate session + storage handles.
                worker_backlog.fetch_sub(1, Ordering::Relaxed);
            }
            warn!("embedding pipeline worker exiting (channel closed)");
        });

        Self {
            sender: tx,
            backlog,
        }
    }

    /// Non-blocking submit; returns `Backpressure` if the queue is full.
    pub fn submit(&self, item: EmbeddingWorkItem) -> Result<(), PipelineError> {
        match self.sender.try_send(item) {
            Ok(()) => {
                self.backlog.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => Err(PipelineError::Backpressure {
                queue: "embedding",
            }),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(PipelineError::WorkerShutdown {
                queue: "embedding",
            }),
        }
    }
}
