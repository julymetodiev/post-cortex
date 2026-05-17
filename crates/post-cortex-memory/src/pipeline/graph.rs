// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Entity-graph work queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use uuid::Uuid;

use super::PipelineError;

/// One unit of graph work.
#[derive(Debug, Clone)]
pub enum GraphWorkItem {
    /// Upsert an entity for a session.
    EntityUpsert {
        /// Session that owns the entity.
        session_id: Uuid,
        /// Human-readable entity name.
        entity_name: String,
        /// Entity type classification (e.g. `"Concept"`, `"Technology"`).
        entity_type: String,
    },
    /// Upsert a directed relation between two entities.
    RelationUpsert {
        /// Session that owns the relationship.
        session_id: Uuid,
        /// Source entity name.
        from: String,
        /// Target entity name.
        to: String,
        /// Relationship type label.
        relation_type: String,
    },
    /// Delete an entity (cascades to its relations on the worker side).
    EntityDelete {
        /// Session that owns the entity.
        session_id: Uuid,
        /// Entity name to remove.
        entity_name: String,
    },
}

/// Bounded MPSC queue feeding the entity-graph worker task.
pub struct GraphQueue {
    sender: mpsc::Sender<GraphWorkItem>,
    backlog: Arc<AtomicUsize>,
}

impl GraphQueue {
    /// Spawn the graph worker on the current Tokio runtime and return the sending handle.
    #[must_use]
    pub fn start(capacity: usize, backlog: Arc<AtomicUsize>) -> Self {
        let (tx, mut rx) = mpsc::channel::<GraphWorkItem>(capacity);
        let worker_backlog = Arc::clone(&backlog);

        tokio::spawn(async move {
            debug!("graph pipeline worker started");
            while let Some(item) = rx.recv().await {
                trace!(?item, "graph pipeline: processing item");
                // Phase 5 stub. Phase 7 wires this to call into
                // crate::memory_system::managers::SimpleGraphManager.
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
