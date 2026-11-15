// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Bounded write pipeline for the work derived from a write.
//!
//! The write itself (the record and its entity-graph merge, one swap and
//! one persist) returns as soon as it is stored; embedding the record and
//! the entities it names is derived work that runs on a worker. This
//! module provides the bounded queues and their worker tasks.
//!
//! ## Layout
//!
//! - [`embedding::EmbeddingQueue`]: a fixed pool of workers, each owning
//!   the sessions that hash to it (so a session's records are embedded in
//!   the order they were written, by one worker at a time). A worker drains
//!   up to [`PipelineConfig::embedding_batch`] queued records at once and
//!   embeds them per session with one `encode_batch`, one vector store
//!   batch, one session persist and one entity pass.
//!
//! ## Backpressure
//!
//! A full queue makes the submitting writer wait for room, at most
//! [`PipelineConfig::submit_wait`]; nothing spawns a task per item. When
//! the wait runs out (or the worker stopped) the submit fails with the
//! item's error and the writer does the work itself (see
//! [`crate::capabilities::Records`]), so the work in flight stays bounded by
//! the queue plus the writers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use thiserror::Error;

use crate::memory_system::MemoryRuntime;

mod embedding;

pub(crate) use embedding::{Embed, EmbeddingQueue, EmbeddingWorkItem};

/// Configuration for the bounded work queues.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Maximum queued embedding requests before writers wait, over all
    /// workers (split evenly, rounded up).
    pub embedding_capacity: usize,
    /// Most queued records a worker embeds in one batch.
    pub embedding_batch: usize,
    /// Embedding workers; a session's records always go to the same one.
    pub embedding_workers: usize,
    /// How long a writer waits for room in a full queue before it does the
    /// work itself.
    pub submit_wait: Duration,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            embedding_capacity: 1_024,
            embedding_batch: 128,
            embedding_workers: 2,
            submit_wait: Duration::from_secs(30),
        }
    }
}

/// Error returned when a queue submission fails.
#[derive(Debug, Error)]
pub(crate) enum PipelineError {
    /// Queue is full — caller should retry, drop, or fall back.
    #[error("pipeline backpressure: {queue} queue is full")]
    Backpressure {
        /// Name of the queue that triggered backpressure.
        queue: &'static str,
    },
    /// Worker task has shut down — the pipeline is no longer usable.
    #[error("pipeline {queue} worker shut down")]
    WorkerShutdown {
        /// Name of the queue whose worker shut down.
        queue: &'static str,
    },
}

/// The bounded pipeline: the queue handles and the gauge of queued work.
///
/// Constructed via [`Self::start`], which spawns the queues' worker
/// tasks. Drop the [`Pipeline`] to signal shutdown: workers exit as soon
/// as their queues drain.
pub(crate) struct Pipeline {
    /// Handle to the embedding work queue.
    embedding: EmbeddingQueue,
    /// How long a submit waits for room.
    submit_wait: Duration,
    /// Sum of queue depths, exposed via [`Pipeline::backlog`] for the
    /// health endpoint.
    backlog: Arc<AtomicUsize>,
}

impl Pipeline {
    /// Spawn worker tasks and return a handle. Uses the current Tokio
    /// runtime via `tokio::spawn`, so it must be called from inside an
    /// async context.
    ///
    /// `system` is shared with the workers so they call the canonical
    /// `*_now` helpers on [`MemoryRuntime`].
    #[must_use]
    pub(crate) fn start(config: PipelineConfig, system: Arc<MemoryRuntime>) -> Self {
        let backlog = Arc::new(AtomicUsize::new(0));
        let embedding = EmbeddingQueue::start(&config, Arc::clone(&backlog), Arc::clone(&system));
        Self {
            embedding,
            submit_wait: config.submit_wait,
            backlog,
        }
    }

    /// Returns a snapshot of the total number of work items currently
    /// queued across all pipeline queues. Used by the health endpoint
    /// to surface backlog pressure.
    #[must_use]
    pub(crate) fn backlog(&self) -> usize {
        self.backlog.load(Ordering::Relaxed)
    }

    /// Queue an embedding request, waiting for room while the queue is
    /// full (at most [`PipelineConfig::submit_wait`]). Fails with
    /// [`PipelineError::Backpressure`] when the wait ran out and
    /// [`PipelineError::WorkerShutdown`] when the worker stopped.
    pub(crate) async fn submit_embedding(
        &self,
        item: EmbeddingWorkItem,
    ) -> Result<(), PipelineError> {
        self.embedding.submit(item, self.submit_wait).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_system::{MemoryRuntime, SystemConfig};
    use uuid::Uuid;

    async fn ephemeral_system() -> (Arc<MemoryRuntime>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = SystemConfig {
            ..Default::default()
        };
        let system = Arc::new(
            crate::memory_system::backends_tests::rocksdb_runtime(&dir, config)
                .await
                .unwrap(),
        );
        (system, dir)
    }

    #[tokio::test]
    async fn pipeline_starts_and_reports_zero_backlog() {
        let (system, _dir) = ephemeral_system().await;
        let pipeline = Pipeline::start(PipelineConfig::default(), system);
        assert_eq!(pipeline.backlog(), 0);
    }

    #[tokio::test]
    async fn pipeline_accepts_and_drains_work() {
        let (system, _dir) = ephemeral_system().await;
        let pipeline = Pipeline::start(PipelineConfig::default(), system);

        pipeline
            .submit_embedding(EmbeddingWorkItem {
                owner: Uuid::new_v4(),
                work: Embed::Record(Uuid::new_v4()),
            })
            .await
            .expect("embedding submit should succeed");

        // Worker tasks consume the items asynchronously; give them a
        // tick to drain.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(pipeline.backlog(), 0, "backlog should drain to 0");
    }
}
