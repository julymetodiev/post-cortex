// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The checkpoint port.

use crate::error::Result;
use async_trait::async_trait;
use uuid::Uuid;

pub use post_cortex_core::session::checkpoint::SessionCheckpoint;

/// Session checkpoints, keyed by session: deleting a session
/// ([`crate::storage::SessionStore::delete_session`]) deletes its checkpoints.
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Store a checkpoint under its session.
    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()>;

    /// The checkpoints of `session_id`, by id; none for a session without
    /// any.
    async fn list_checkpoints(&self, session_id: Uuid) -> Result<Vec<SessionCheckpoint>>;
}
