// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The radar-dismissal port.

use crate::error::Result;
use async_trait::async_trait;
use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};
use uuid::Uuid;

/// Storage for radar dismissals (see
/// [`post_cortex_core::core::dismissal`]), per workspace.
#[async_trait]
pub trait DismissalStore: Send + Sync {
    /// Store `dismissal` for `workspace_id`, replacing one of the same kind
    /// and key.
    async fn put_dismissal(&self, workspace_id: Uuid, dismissal: &RadarDismissal) -> Result<()>;

    /// Delete the dismissal of `kind` and `key`; returns whether it existed.
    async fn delete_dismissal(
        &self,
        workspace_id: Uuid,
        kind: RadarItemKind,
        key: &str,
    ) -> Result<bool>;

    /// The dismissals of `workspace_id`, by kind and key.
    async fn list_dismissals(&self, workspace_id: Uuid) -> Result<Vec<RadarDismissal>>;
}
