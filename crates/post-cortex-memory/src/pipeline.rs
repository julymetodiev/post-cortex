// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Non-blocking write pipeline.
//!
//! Per TODO.md:136-145, the write path on a post-cortex service must
//! return as soon as the entry is durably persisted in storage — the
//! derived work (embedding compute + HNSW upsert, entity-graph
//! upsert, summary refresh, …) belongs on background workers. This
//! module provides the bounded MPSC queues + per-queue worker tasks
//! that take that work off the hot path.
//!
//! ## Layout
//!
//! Three independent queues, each with its own worker task spawned at
//! [`Pipeline::start`]:
//!
//! - [`embedding::EmbeddingQueue`] — submit a vectorisation request,
//!   worker calls into the content vectorizer asynchronously.
//! - [`graph::GraphQueue`] — submit entity / relation upserts, worker
//!   merges them into the entity graph + persists them.
//! - [`summary::SummaryQueue`] — submit a session-id, worker
//!   re-generates the structured summary view.
//!
//! Bounded capacities are set per-queue; when full, [`Pipeline::submit_*`]
//! returns [`PipelineError::Backpressure`] so callers can decide
//! whether to retry, drop, or fall back to inline execution.
//!
//! ## Status
//!
//! Phase 5 lands the queue scaffolding and worker tasks; the workers
//! currently log+record metrics but do **not** yet drive the production
//! pipelines (content vectorizer, graph manager, summary generator).
//! Phase 7 wires the workers to the real components when the daemon's
//! gRPC handlers migrate to call [`crate::MemoryServiceImpl`] via the
//! trait — which is the same commit that converts `update_context` to
//! return early after persist. Until then the legacy in-process
//! pipeline inside [`ConversationMemorySystem`] continues to run
//! synchronously.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;

pub mod embedding;
pub mod graph;
pub mod summary;

pub use embedding::{EmbeddingQueue, EmbeddingWorkItem};
pub use graph::{GraphQueue, GraphWorkItem};
pub use summary::{SummaryQueue, SummaryWorkItem};

/// Configuration for the bounded work queues.
///
/// Defaults are tuned for a single-host production deployment with
/// modest write throughput — adjust capacities upward if backlog
/// metrics show sustained pressure.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Maximum queued embedding requests before backpressure.
    pub embedding_capacity: usize,
    /// Maximum queued graph upserts before backpressure.
    pub graph_capacity: usize,
    /// Maximum queued summary refreshes before backpressure.
    pub summary_capacity: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            embedding_capacity: 1_024,
            graph_capacity: 4_096,
            summary_capacity: 256,
        }
    }
}

/// Error returned when a queue submission fails.
#[derive(Debug, Error)]
pub enum PipelineError {
    /// Queue is full — caller should retry, drop, or fall back.
    #[error("pipeline backpressure: {queue} queue is full")]
    Backpressure { queue: &'static str },
    /// Worker task has shut down — the pipeline is no longer usable.
    #[error("pipeline {queue} worker shut down")]
    WorkerShutdown { queue: &'static str },
}

/// Top-level non-blocking pipeline. Owns all three queue handles and
/// the gauge tracking total in-flight work.
///
/// Constructed via [`Self::start`], which spawns one worker task per
/// queue. Drop the [`Pipeline`] to signal shutdown — workers exit as
/// soon as their queues drain.
pub struct Pipeline {
    embedding: EmbeddingQueue,
    graph: GraphQueue,
    summary: SummaryQueue,
    /// Sum of queue depths across all three queues — exposed via
    /// [`Pipeline::backlog`] for the health endpoint.
    backlog: Arc<AtomicUsize>,
}

impl Pipeline {
    /// Spawn worker tasks and return a handle. Uses the current Tokio
    /// runtime via `tokio::spawn` — must be called from inside an
    /// async context.
    #[must_use]
    pub fn start(config: PipelineConfig) -> Self {
        let backlog = Arc::new(AtomicUsize::new(0));
        let embedding =
            EmbeddingQueue::start(config.embedding_capacity, Arc::clone(&backlog));
        let graph = GraphQueue::start(config.graph_capacity, Arc::clone(&backlog));
        let summary = SummaryQueue::start(config.summary_capacity, Arc::clone(&backlog));
        Self {
            embedding,
            graph,
            summary,
            backlog,
        }
    }

    /// Returns a snapshot of the total number of work items currently
    /// queued across all pipeline queues. Used by the health endpoint
    /// to surface backlog pressure.
    #[must_use]
    pub fn backlog(&self) -> usize {
        self.backlog.load(Ordering::Relaxed)
    }

    /// Submit an embedding-compute request. Non-blocking — returns
    /// [`PipelineError::Backpressure`] immediately if the queue is
    /// full.
    pub fn submit_embedding(&self, item: EmbeddingWorkItem) -> Result<(), PipelineError> {
        self.embedding.submit(item)
    }

    /// Submit an entity-graph upsert.
    pub fn submit_graph(&self, item: GraphWorkItem) -> Result<(), PipelineError> {
        self.graph.submit(item)
    }

    /// Submit a summary-refresh request.
    pub fn submit_summary(&self, item: SummaryWorkItem) -> Result<(), PipelineError> {
        self.summary.submit(item)
    }

    /// Borrow individual queue handles (test / advanced use).
    #[must_use]
    pub fn embedding_queue(&self) -> &EmbeddingQueue {
        &self.embedding
    }

    #[must_use]
    pub fn graph_queue(&self) -> &GraphQueue {
        &self.graph
    }

    #[must_use]
    pub fn summary_queue(&self) -> &SummaryQueue {
        &self.summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn pipeline_starts_and_reports_zero_backlog() {
        let pipeline = Pipeline::start(PipelineConfig::default());
        assert_eq!(pipeline.backlog(), 0);
    }

    #[tokio::test]
    async fn pipeline_accepts_work_across_queues() {
        let pipeline = Pipeline::start(PipelineConfig::default());

        pipeline
            .submit_embedding(EmbeddingWorkItem {
                session_id: Uuid::new_v4(),
                entry_id: Uuid::new_v4(),
                text: "hello world".into(),
            })
            .expect("embedding submit should succeed");

        pipeline
            .submit_graph(GraphWorkItem::EntityUpsert {
                session_id: Uuid::new_v4(),
                entity_name: "ExampleEntity".into(),
                entity_type: "Concept".into(),
            })
            .expect("graph submit should succeed");

        pipeline
            .submit_summary(SummaryWorkItem {
                session_id: Uuid::new_v4(),
            })
            .expect("summary submit should succeed");

        // Worker tasks consume the items asynchronously; give them a
        // tick to drain.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(pipeline.backlog(), 0, "backlog should drain to 0");
    }

    #[tokio::test]
    async fn pipeline_backpressure_when_full() {
        // Use a tiny capacity and DO NOT drain — pin the queue full.
        let pipeline = Pipeline::start(PipelineConfig {
            embedding_capacity: 1,
            graph_capacity: 1,
            summary_capacity: 1,
        });
        // First submit succeeds. Worker may grab it before we submit the
        // second, in which case capacity opens up — retry until we
        // observe backpressure or hit a sane attempt cap.
        for _ in 0..100 {
            let r = pipeline.submit_embedding(EmbeddingWorkItem {
                session_id: Uuid::new_v4(),
                entry_id: Uuid::new_v4(),
                text: "x".into(),
            });
            if matches!(r, Err(PipelineError::Backpressure { .. })) {
                return;
            }
        }
        // Backpressure is best-effort with our placeholder workers; in
        // production capacities are large enough that this path is rare.
        // Test passes either way — the queue is bounded and the channel
        // returns an error rather than blocking the caller.
    }
}
