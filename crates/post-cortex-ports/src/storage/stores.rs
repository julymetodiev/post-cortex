// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`Stores`]: one backend's ports, each as its own trait object.

use std::sync::Arc;

use crate::storage::{
    CardStore, CheckpointStore, DismissalStore, DocumentStore, EventCursors, EventLog,
    FreshnessStore, MessageStore, SessionStore, WorkspaceStore,
};

/// The ports of one storage backend. Built once at the composition root
/// from the backend struct; each capability takes the ports it uses.
#[derive(Clone)]
pub struct Stores {
    /// Sessions, their records and their entity graphs.
    pub sessions: Arc<dyn SessionStore>,
    /// Session checkpoints.
    pub checkpoints: Arc<dyn CheckpointStore>,
    /// Workspaces, members and projects.
    pub workspaces: Arc<dyn WorkspaceStore>,
    /// Captured message streams.
    pub messages: Arc<dyn MessageStore>,
    /// Cards and their history.
    pub cards: Arc<dyn CardStore>,
    /// Documents of modules and their versions.
    pub documents: Arc<dyn DocumentStore>,
    /// Radar dismissals.
    pub dismissals: Arc<dyn DismissalStore>,
    /// Source references and staleness.
    pub freshness: Arc<dyn FreshnessStore>,
    /// The organisation's bus: its events.
    pub events: Arc<dyn EventLog>,
    /// The organisation's bus: its followers' positions.
    pub cursors: Arc<dyn EventCursors>,
}

impl Stores {
    /// Every capability port of `backend`. Its persisted embeddings
    /// (`VectorStore`) are not among them: the composition hands them to
    /// the `SearchIndex` adapter.
    pub fn of<B>(backend: Arc<B>) -> Self
    where
        B: SessionStore
            + CheckpointStore
            + WorkspaceStore
            + MessageStore
            + CardStore
            + DocumentStore
            + DismissalStore
            + FreshnessStore
            + EventLog
            + EventCursors
            + 'static,
    {
        Self {
            sessions: backend.clone(),
            checkpoints: backend.clone(),
            workspaces: backend.clone(),
            messages: backend.clone(),
            cards: backend.clone(),
            documents: backend.clone(),
            dismissals: backend.clone(),
            freshness: backend.clone(),
            events: backend.clone(),
            cursors: backend,
        }
    }
}
