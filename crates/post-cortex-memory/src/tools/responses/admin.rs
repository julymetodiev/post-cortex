// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Admin responses: health, vectorization, checkpoints.

use crate::capabilities::Health;
use serde::Serialize;

/// The one health payload (MCP `admin health`, REST `/health`): the typed
/// [`Health`] plus the legacy `total_updates` (the same count as
/// `total_requests`, kept for clients that read it).
#[derive(Debug, Clone, Serialize)]
pub struct HealthPayload {
    /// The health report.
    #[serde(flatten)]
    pub health: Health,
    /// The same count as `total_requests`.
    pub total_updates: u64,
}

impl From<Health> for HealthPayload {
    fn from(health: Health) -> Self {
        Self {
            total_updates: health.total_requests,
            health,
        }
    }
}

/// A session's records embedded.
#[derive(Debug, Clone, Serialize)]
pub struct Vectorized {
    /// The session.
    pub session_id: String,
    /// Vectors written.
    pub vectors_created: usize,
}

/// A checkpoint written.
#[derive(Debug, Clone, Serialize)]
pub struct Checkpoint {
    /// Its id.
    pub checkpoint_id: String,
}
