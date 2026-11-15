// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Session checkpoints: a session's state at one point in time, stored by
//! the checkpoint port and carried by export archives.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::context_update::{CodeReference, ContextUpdate};
use crate::core::structured_context::StructuredContext;
use crate::session::active_session::ChangeRecord;

/// Point-in-time snapshot of a session, used for restore and analysis.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionCheckpoint {
    /// Checkpoint ID.
    pub id: Uuid,
    /// Session this checkpoint was taken from.
    pub session_id: Uuid,
    /// Timestamp the checkpoint was created.
    pub created_at: chrono::DateTime<chrono::Utc>,

    // Complete context snapshot
    /// Snapshot of the session's structured context at checkpoint time.
    pub structured_context: StructuredContext,
    /// Recent context updates included in the snapshot.
    pub recent_updates: Vec<ContextUpdate>,
    /// Code references grouped by file path.
    pub code_references: HashMap<String, Vec<CodeReference>>,
    /// Change history records included in the snapshot.
    pub change_history: Vec<ChangeRecord>,

    // Metadata
    /// Total number of updates the session contained at checkpoint time.
    pub total_updates: usize,
    /// Computed context-quality score in `[0.0, 1.0]`.
    pub context_quality_score: f32,
    /// Compression ratio achieved by the snapshot.
    pub compression_ratio: f32,
}
