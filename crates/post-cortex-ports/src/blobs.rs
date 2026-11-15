// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The blob port (ARCHITECTURE section 5, `BlobStore`; W24): bytes by
//! their SHA-256, per workspace (key `<workspace>/<sha256>`), apart from
//! the store of records and documents, so a storage backend keeps no
//! bytes. Its adapter is S3 (`post-cortex-blobs`).

use async_trait::async_trait;
use post_cortex_core::core::blob::{BlobId, BlobInfo};
use uuid::Uuid;

use crate::error::Result;

/// A blob of a workspace: what it is, and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    /// What it is.
    pub info: BlobInfo,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// The blobs of workspaces.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Keep `blob` in `workspace_id`; one the workspace has already is not
    /// written again. What is kept: the stored blob's info.
    async fn put_blob(&self, workspace_id: Uuid, blob: Blob) -> Result<BlobInfo>;

    /// Blob `id` of `workspace_id`, none when it has none.
    async fn get_blob(&self, workspace_id: Uuid, id: &BlobId) -> Result<Option<Blob>>;

    /// What blob `id` of `workspace_id` is, none when it has none.
    async fn stat_blob(&self, workspace_id: Uuid, id: &BlobId) -> Result<Option<BlobInfo>>;

    /// Every blob of `workspace_id`, in no particular order.
    async fn list_blobs(&self, workspace_id: Uuid) -> Result<Vec<BlobInfo>>;

    /// Delete blob `id` of `workspace_id`. Whether it existed.
    async fn delete_blob(&self, workspace_id: Uuid, id: &BlobId) -> Result<bool>;
}
