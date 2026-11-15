// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding work queue: bounded, sharded by session over a fixed number of
//! workers, each draining a batch at a time.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use tokio::sync::mpsc;
use tracing::{debug, error, trace, warn};
use uuid::Uuid;

use crate::memory_system::MemoryRuntime;
use crate::memory_system::derived_work::{DerivedWork, DerivedWorkGuard};
use crate::pipeline::{PipelineConfig, PipelineError};

/// What one unit of embedding work embeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Embed {
    /// A record and the entities it names
    /// (`MemoryRuntime::vectorize_updates_now`).
    Record(Uuid),
    /// Captured turns, by position (`MemoryRuntime::vectorize_messages_now`).
    Messages(Vec<u64>),
    /// The latest version of a workspace's document
    /// (`MemoryRuntime::vectorize_documents_now`).
    Document(Uuid),
}

/// One unit of embedding work of `owner`.
#[derive(Debug, Clone)]
pub(crate) struct EmbeddingWorkItem {
    /// The session (records, turns) or the workspace (documents) that owns
    /// the content.
    pub owner: Uuid,
    /// What to embed.
    pub work: Embed,
}

/// A queued item with its derived-work guard, dropped once the worker is
/// done with it (see `MemoryRuntime::flush_all`).
type Job = (EmbeddingWorkItem, DerivedWorkGuard);

/// Handle to the bounded channels feeding the embedding workers.
pub(crate) struct EmbeddingQueue {
    /// One channel per worker; an owner always goes to the same one, so
    /// its content is embedded in the order it was written and two workers
    /// never embed one owner's content at once.
    shards: Vec<mpsc::Sender<Job>>,
    backlog: Arc<AtomicUsize>,
    work: Arc<DerivedWork>,
}

/// The work of one owner in a drained batch: its records, its turns and
/// its documents.
#[derive(Debug, PartialEq, Eq)]
struct OwnerBatch {
    owner: Uuid,
    records: Vec<Uuid>,
    messages: Vec<u64>,
    documents: Vec<Uuid>,
}

/// The work of `jobs` by owner, owners and items in queue order.
fn by_owner(jobs: &[Job]) -> Vec<OwnerBatch> {
    let mut groups: Vec<OwnerBatch> = Vec::new();
    for (item, _) in jobs {
        let i = match groups.iter().position(|g| g.owner == item.owner) {
            Some(i) => i,
            None => {
                groups.push(OwnerBatch {
                    owner: item.owner,
                    records: Vec::new(),
                    messages: Vec::new(),
                    documents: Vec::new(),
                });
                groups.len() - 1
            }
        };
        let group = &mut groups[i];
        match &item.work {
            Embed::Record(id) => group.records.push(*id),
            Embed::Messages(seqs) => group.messages.extend(seqs),
            Embed::Document(id) if !group.documents.contains(id) => group.documents.push(*id),
            Embed::Document(_) => {}
        }
    }
    groups
}

/// Embed one drained batch: per owner it holds, one call for its records,
/// one for its turns and one for its documents.
async fn embed_batch(system: &MemoryRuntime, jobs: &[Job]) {
    for batch in by_owner(jobs) {
        let owner = batch.owner;
        if !batch.records.is_empty() {
            let result = system.vectorize_updates_now(owner, &batch.records).await;
            log_batch(owner, "records", batch.records.len(), result);
        }
        if !batch.messages.is_empty() {
            let result = system.vectorize_messages_now(owner, &batch.messages).await;
            log_batch(owner, "messages", batch.messages.len(), result);
        }
        if !batch.documents.is_empty() {
            let result = system
                .vectorize_documents_now(owner, &batch.documents)
                .await;
            log_batch(owner, "documents", batch.documents.len(), result);
        }
    }
}

fn log_batch(
    owner: Uuid,
    kind: &str,
    items: usize,
    result: Result<usize, crate::memory_system::RuntimeError>,
) {
    match result {
        Ok(count) => debug!(
            %owner,
            kind,
            items,
            vectorised = count,
            "embedding pipeline: batch done"
        ),
        Err(e) => warn!(
            %owner,
            kind,
            items,
            error = %e,
            "embedding pipeline: vectorise failed (non-fatal)"
        ),
    }
}

impl EmbeddingQueue {
    /// Spawn [`PipelineConfig::embedding_workers`] workers on the current
    /// Tokio runtime and return the sending handle. The queue holds at most
    /// [`PipelineConfig::embedding_capacity`] items, split evenly over the
    /// workers; a worker takes up to [`PipelineConfig::embedding_batch`]
    /// queued items at a time.
    ///
    /// `system` gives the workers the canonical
    /// [`MemoryRuntime::vectorize_updates_now`].
    #[must_use]
    pub(crate) fn start(
        config: &PipelineConfig,
        backlog: Arc<AtomicUsize>,
        system: Arc<MemoryRuntime>,
    ) -> Self {
        let workers = config.embedding_workers.max(1);
        let capacity = config.embedding_capacity.div_ceil(workers).max(1);
        let batch = config.embedding_batch.max(1);
        let work = Arc::clone(&system.derived_work);
        let shards = (0..workers)
            .map(|worker| {
                let (tx, rx) = mpsc::channel::<Job>(capacity);
                tokio::spawn(run_worker(
                    worker,
                    rx,
                    batch,
                    Arc::clone(&backlog),
                    Arc::clone(&system),
                ));
                tx
            })
            .collect();
        Self {
            shards,
            backlog,
            work,
        }
    }

    /// Queue `item`, waiting at most `wait` for room while its shard is
    /// full. Fails with [`PipelineError::Backpressure`] when the wait ran
    /// out and [`PipelineError::WorkerShutdown`] when the worker stopped;
    /// the item is not queued then.
    pub(crate) async fn submit(
        &self,
        item: EmbeddingWorkItem,
        wait: std::time::Duration,
    ) -> Result<(), PipelineError> {
        let shard = &self.shards[shard_of(item.owner, self.shards.len())];
        let job = self.work.begin(item.owner);
        // Counted before the send so the worker never decrements first.
        self.backlog.fetch_add(1, Ordering::Relaxed);
        let sent = shard.send_timeout((item, job), wait).await;
        if sent.is_err() {
            self.backlog.fetch_sub(1, Ordering::Relaxed);
        }
        match sent {
            Ok(()) => Ok(()),
            Err(mpsc::error::SendTimeoutError::Timeout(_)) => {
                Err(PipelineError::Backpressure { queue: "embedding" })
            }
            Err(mpsc::error::SendTimeoutError::Closed(_)) => {
                Err(PipelineError::WorkerShutdown { queue: "embedding" })
            }
        }
    }
}

/// The worker of `owner` among `workers`.
fn shard_of(owner: Uuid, workers: usize) -> usize {
    (owner.as_u128() % workers as u128) as usize
}

/// Drain `rx` up to `batch` items at a time until every sender is gone.
async fn run_worker(
    worker: usize,
    mut rx: mpsc::Receiver<Job>,
    batch: usize,
    backlog: Arc<AtomicUsize>,
    system: Arc<MemoryRuntime>,
) {
    debug!(worker, "embedding pipeline worker started");
    let mut jobs: Vec<Job> = Vec::with_capacity(batch);
    while rx.recv_many(&mut jobs, batch).await > 0 {
        trace!(worker, items = jobs.len(), "embedding pipeline: batch");
        // A batch that panics is lost, not the worker: its records stay
        // without vectors (found by the next search's missing-records pass)
        // and the worker takes the next batch.
        if AssertUnwindSafe(embed_batch(&system, &jobs))
            .catch_unwind()
            .await
            .is_err()
        {
            error!(
                worker,
                items = jobs.len(),
                "embedding pipeline: batch panicked"
            );
        }
        let done = jobs.len();
        // Drops the guards: the batch's persist is queued by now.
        jobs.clear();
        backlog.fetch_sub(done, Ordering::Relaxed);
    }
    // Every sender is gone only when the pipeline is dropped: the
    // instance is stopping, which is not a fault.
    debug!(worker, "embedding pipeline worker stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_guard(work: &Arc<DerivedWork>, session_id: Uuid) -> DerivedWorkGuard {
        work.begin(session_id)
    }

    #[test]
    fn a_batch_is_grouped_by_owner_in_queue_order() {
        let work_counter = Arc::new(DerivedWork::default());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let ids: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
        let items = [
            (a, Embed::Record(ids[0])),
            (b, Embed::Record(ids[1])),
            (a, Embed::Messages(vec![3, 4])),
            (a, Embed::Record(ids[2])),
            (b, Embed::Record(ids[3])),
            (b, Embed::Document(ids[0])),
            (b, Embed::Document(ids[0])),
        ];
        let jobs: Vec<Job> = items
            .into_iter()
            .map(|(owner, work)| {
                (
                    EmbeddingWorkItem { owner, work },
                    work_guard(&work_counter, owner),
                )
            })
            .collect();
        assert_eq!(
            by_owner(&jobs),
            vec![
                OwnerBatch {
                    owner: a,
                    records: vec![ids[0], ids[2]],
                    messages: vec![3, 4],
                    documents: vec![],
                },
                OwnerBatch {
                    owner: b,
                    records: vec![ids[1], ids[3]],
                    messages: vec![],
                    documents: vec![ids[0]],
                },
            ]
        );
    }

    #[test]
    fn a_session_always_maps_to_one_worker() {
        let sid = Uuid::new_v4();
        for workers in 1..8 {
            let shard = shard_of(sid, workers);
            assert!(shard < workers);
            assert_eq!(shard, shard_of(sid, workers));
        }
    }
}
