// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Handing the derived work of a write, a message append or a document
//! version (embeddings) to the bounded pipeline, and summarising
//! backpressure in the log. Records, Messages and Documents submit here.

use std::sync::Arc;

use crate::memory_system::MemoryRuntime;
use crate::pipeline::{Embed, EmbeddingWorkItem, Pipeline, PipelineError};
use tracing::warn;
use uuid::Uuid;

/// The pipeline, the runtime its fallback runs on and the backpressure
/// log: what a capability needs to submit derived work.
#[derive(Clone)]
pub(crate) struct DerivedQueue {
    pipeline: Arc<Pipeline>,
    rt: Arc<MemoryRuntime>,
    backpressure: Arc<BackpressureLog>,
}

impl DerivedQueue {
    pub(crate) fn new(pipeline: Arc<Pipeline>, rt: Arc<MemoryRuntime>) -> Self {
        Self {
            pipeline,
            rt,
            backpressure: Arc::default(),
        }
    }

    /// Hand stored content's embedding to the bounded pipeline. A full
    /// queue makes the caller wait for room (backpressure); when the wait
    /// runs out or the worker stopped, the caller embeds it itself, so the
    /// work is never dropped and never runs on a task spawned per item. The
    /// graph merge of a write is not derived work: it is part of the write.
    pub(crate) async fn submit(&self, owner: Uuid, work: Embed) {
        let item = EmbeddingWorkItem {
            owner,
            work: work.clone(),
        };
        if let Err(e) = self.pipeline.submit_embedding(item).await {
            log_pipeline_submit(&self.backpressure, "embedding", owner, &work, e);
            self.embed_now(owner, work).await;
        }
    }

    async fn embed_now(&self, owner: Uuid, work: Embed) {
        let result = match &work {
            Embed::Record(entry_id) => self.rt.vectorize_update_now(owner, *entry_id).await,
            Embed::Messages(seqs) => self.rt.vectorize_messages_now(owner, seqs).await,
            Embed::Document(id) => self.rt.vectorize_documents_now(owner, &[*id]).await,
        };
        if let Err(e) = result {
            warn!(%owner, ?work, error = %e, "inline vectorization failed");
        }
    }
}

/// A queue that stays full for the whole wait is expected during heavy bulk
/// writes (the writer then does the work itself, so nothing is lost): each
/// case is logged at debug level and summed into one warning per queue at
/// most every [`BACKPRESSURE_WARN_EVERY`]. A stopped worker is a warning
/// every time.
fn log_pipeline_submit(
    backpressure: &BackpressureLog,
    queue: &'static str,
    owner: Uuid,
    work: &Embed,
    err: PipelineError,
) {
    match err {
        PipelineError::Backpressure { .. } => {
            tracing::debug!(queue, %owner, ?work, "pipeline queue stayed full; the writer does the work");
            if let Some((count, since)) = backpressure.note(queue) {
                warn!(
                    queue,
                    count,
                    seconds = since.as_secs(),
                    "pipeline queue stayed full: writers did {count} work items themselves (bulk ingest?)"
                );
            }
        }
        err @ PipelineError::WorkerShutdown { .. } => warn!(
            queue,
            %owner,
            ?work,
            error = %err,
            "pipeline submission failed (non-fatal, the writer does the work)"
        ),
    }
}

/// How often a full pipeline queue is reported.
const BACKPRESSURE_WARN_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Per-queue count of full-queue fallbacks since the last warning.
#[derive(Default)]
pub(crate) struct BackpressureLog {
    queues: parking_lot::Mutex<Vec<(&'static str, std::time::Instant, u64)>>,
}

impl BackpressureLog {
    /// Count one fallback for `queue`; returns the count and the time it
    /// covers when a warning is due (the first case, then at most every
    /// [`BACKPRESSURE_WARN_EVERY`]).
    fn note(&self, queue: &'static str) -> Option<(u64, std::time::Duration)> {
        let now = std::time::Instant::now();
        let mut queues = self.queues.lock();
        let Some(entry) = queues.iter_mut().find(|(q, _, _)| *q == queue) else {
            queues.push((queue, now, 0));
            return Some((1, std::time::Duration::ZERO));
        };
        entry.2 += 1;
        let since = now.duration_since(entry.1);
        if since < BACKPRESSURE_WARN_EVERY {
            return None;
        }
        let count = entry.2;
        *entry = (queue, now, 0);
        Some((count, since))
    }
}

#[cfg(test)]
mod tests;
