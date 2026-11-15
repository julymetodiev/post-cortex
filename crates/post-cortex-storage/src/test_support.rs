// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Test helpers for the suites that exercise a backend whole (the per-port
//! cases are in `conformance`).

use post_cortex_ports::storage::{
    CardStore, CheckpointStore, DismissalStore, FreshnessStore, MessageStore, SessionStore,
    VectorStore, WorkspaceStore,
};

/// Every port, as one backend struct implements them.
pub(crate) trait Backend:
    SessionStore
    + CheckpointStore
    + WorkspaceStore
    + VectorStore
    + MessageStore
    + CardStore
    + DismissalStore
    + FreshnessStore
    + Clone
    + 'static
{
}

impl<T> Backend for T where
    T: SessionStore
        + CheckpointStore
        + WorkspaceStore
        + VectorStore
        + MessageStore
        + CardStore
        + DismissalStore
        + FreshnessStore
        + Clone
        + 'static
{
}
