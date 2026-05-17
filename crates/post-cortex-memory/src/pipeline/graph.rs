// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Entity-graph work queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use post_cortex_core::core::context_update::ContextUpdate;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use uuid::Uuid;

use super::PipelineError;
use crate::memory_system::ConversationMemorySystem;

/// One unit of graph work.
///
/// The single `ApplyUpdate` variant matches the canonical entity-graph
/// flow: each [`ContextUpdate`] carries its own typed entities + relations
/// and the worker applies them atomically via
/// [`ConversationMemorySystem::apply_entity_graph_update_now`]. Per-entity
/// or per-relation deltas can be added as new variants if a use case
/// emerges, but no path through the codebase currently emits them — the
/// canonical write at `MemoryServiceImpl::update_context` always submits a
/// full update.
#[derive(Debug, Clone)]
pub enum GraphWorkItem {
    /// Apply a full [`ContextUpdate`]'s entity + relation set to the
    /// session graph.
    ApplyUpdate {
        /// Session that owns the entities / relations.
        session_id: Uuid,
        /// The context-update payload to merge into the entity graph.
        update: ContextUpdate,
    },
}

impl GraphWorkItem {
    /// Convenience: build an `ApplyUpdate` work item.
    #[must_use]
    pub fn apply_update(session_id: Uuid, update: ContextUpdate) -> Self {
        Self::ApplyUpdate { session_id, update }
    }
}

/// Bounded MPSC queue feeding the entity-graph worker task.
pub struct GraphQueue {
    sender: mpsc::Sender<GraphWorkItem>,
    backlog: Arc<AtomicUsize>,
}

impl GraphQueue {
    /// Spawn the graph worker on the current Tokio runtime and return the sending handle.
    ///
    /// `system` lets the worker reach the canonical
    /// [`ConversationMemorySystem::apply_entity_graph_update_now`] helper
    /// — extracting that body into a public method is how the worker
    /// avoids reimplementing the load → mutate → CAS → persist flow.
    #[must_use]
    pub fn start(
        capacity: usize,
        backlog: Arc<AtomicUsize>,
        system: Arc<ConversationMemorySystem>,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<GraphWorkItem>(capacity);
        let worker_backlog = Arc::clone(&backlog);

        tokio::spawn(async move {
            debug!("graph pipeline worker started");
            while let Some(item) = rx.recv().await {
                match &item {
                    GraphWorkItem::ApplyUpdate { session_id, update } => {
                        trace!(
                            session_id = %session_id,
                            update_id = %update.id,
                            entities = update.creates_entities.len(),
                            relations = update.creates_relationships.len(),
                            "graph pipeline: applying update"
                        );
                        if let Err(e) = system
                            .apply_entity_graph_update_now(*session_id, update.clone())
                            .await
                        {
                            warn!(
                                session_id = %session_id,
                                error = %e,
                                "graph pipeline: apply failed (non-fatal)"
                            );
                        }
                    }
                }
                worker_backlog.fetch_sub(1, Ordering::Relaxed);
            }
            warn!("graph pipeline worker exiting (channel closed)");
        });

        Self {
            sender: tx,
            backlog,
        }
    }

    /// Non-blocking submit; returns [`PipelineError::Backpressure`] if the queue is full.
    pub fn submit(&self, item: GraphWorkItem) -> Result<(), PipelineError> {
        match self.sender.try_send(item) {
            Ok(()) => {
                self.backlog.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                Err(PipelineError::Backpressure { queue: "graph" })
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(PipelineError::WorkerShutdown { queue: "graph" })
            }
        }
    }
}
