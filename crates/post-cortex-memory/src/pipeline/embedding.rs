// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding work queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use uuid::Uuid;

use super::PipelineError;
use crate::memory_system::ConversationMemorySystem;

/// One unit of embedding work.
///
/// `session_id` is the canonical identifier; the worker re-derives the
/// "latest update to vectorise" from the session's hot context (matching
/// the legacy `spawn_background_vectorization` path). `entry_id` + `text`
/// are kept for forward compatibility with per-entry vectorisation
/// (`ContentVectorizer::vectorize_one`-style API, not yet implemented).
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
    ///
    /// `system` gives the worker access to the canonical
    /// [`ConversationMemorySystem::vectorize_latest_update_now`] helper.
    /// When the `embeddings` feature is disabled the worker still runs
    /// but reports each item as a no-op so callers see deterministic
    /// backlog drain regardless of build features.
    #[must_use]
    pub fn start(
        capacity: usize,
        backlog: Arc<AtomicUsize>,
        system: Arc<ConversationMemorySystem>,
    ) -> Self {
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

                #[cfg(feature = "embeddings")]
                {
                    match system.vectorize_latest_update_now(item.session_id).await {
                        Ok(0) => {
                            trace!(
                                session_id = %item.session_id,
                                "embedding pipeline: nothing to vectorise (auto-disabled or no fresh update)"
                            );
                        }
                        Ok(count) => {
                            debug!(
                                session_id = %item.session_id,
                                vectorised = count,
                                "embedding pipeline: vectorisation complete"
                            );
                        }
                        Err(e) => warn!(
                            session_id = %item.session_id,
                            error = %e,
                            "embedding pipeline: vectorise failed (non-fatal)"
                        ),
                    }
                }

                #[cfg(not(feature = "embeddings"))]
                {
                    let _ = &system; // silence unused warning when feature is off
                }

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
