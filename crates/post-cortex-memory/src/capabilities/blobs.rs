// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Blobs (ARCHITECTURE section 4, Blobs; W24): the bytes of the images a
//! module's documents name, by their SHA-256, per workspace. A put checks
//! the bytes (an image known by its first bytes, at most 10 MB) and answers
//! what it is; a blob is kept while some version of a document of its
//! workspace names it (the sweep deletes the others, once they are older
//! than its cutoff) and is deleted with its workspace.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use post_cortex_core::core::blob::{self, BlobId, BlobInfo, BlobRefused};
use post_cortex_ports::{Blob, BlobStore, DocumentKinds};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::capabilities::documents::{DocumentError, Documents};
use crate::memory_system::{RuntimeError, WorkspaceManager};

/// Why a blob operation failed.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// The workspace does not exist.
    #[error("Workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    /// The workspace has no such blob.
    #[error("Blob {0} not found")]
    NotFound(BlobId),
    /// The bytes or the id are refused.
    #[error("Invalid argument: {0}")]
    Refused(#[from] BlobRefused),
    /// This memory instance keeps no blobs.
    #[error("no blob store is configured")]
    Unconfigured,
    /// Reading what the documents name failed.
    #[error(transparent)]
    Documents(#[from] DocumentError),
    /// The blob store failed.
    #[error("Internal error: {0}")]
    Storage(#[from] RuntimeError),
}

fn storage(e: post_cortex_ports::PortError) -> BlobError {
    BlobError::Storage(e.into())
}

/// What a sweep did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Swept {
    /// Blobs deleted: no version of a document names them.
    pub deleted: usize,
    /// Workspaces left as they are: a document there does not read, so
    /// what it names is not known.
    pub skipped: usize,
}

/// The Blobs capability service.
#[derive(Clone)]
pub struct Blobs {
    store: Option<Arc<dyn BlobStore>>,
    documents: Documents,
    workspaces: Arc<WorkspaceManager>,
}

impl Blobs {
    pub(crate) fn new(
        store: Option<Arc<dyn BlobStore>>,
        documents: Documents,
        workspaces: Arc<WorkspaceManager>,
    ) -> Self {
        Self {
            store,
            documents,
            workspaces,
        }
    }

    /// Keep `bytes` in `workspace_id`, of media type `declared` (empty: an
    /// image known by its first bytes); what they are, with their SHA-256.
    /// Bytes the workspace has already are not written again.
    ///
    /// # Errors
    /// `WorkspaceNotFound`; `Refused` for bytes that are no image kept, a
    /// media type kept or over the limit; `Unconfigured`; the store failed.
    pub async fn put(
        &self,
        workspace_id: Uuid,
        bytes: Vec<u8>,
        declared: &str,
    ) -> Result<BlobInfo, BlobError> {
        let store = self.store(workspace_id)?;
        let image = blob::blob(&bytes, declared)?;
        let id = BlobId::of_digest(&Sha256::digest(&bytes).into());
        let info = BlobInfo {
            id,
            size: bytes.len() as u64,
            image,
            put_at: Utc::now(),
        };
        store
            .put_blob(workspace_id, Blob { info, bytes })
            .await
            .map_err(storage)
    }

    /// Blob `id` of `workspace_id`.
    ///
    /// # Errors
    /// `WorkspaceNotFound`; `NotFound`; `Unconfigured`; the store failed.
    pub async fn get(&self, workspace_id: Uuid, id: &BlobId) -> Result<Blob, BlobError> {
        self.store(workspace_id)?
            .get_blob(workspace_id, id)
            .await
            .map_err(storage)?
            .ok_or_else(|| BlobError::NotFound(id.clone()))
    }

    /// What blob `id` of `workspace_id` is.
    ///
    /// # Errors
    /// `WorkspaceNotFound`; `NotFound`; `Unconfigured`; the store failed.
    pub async fn stat(&self, workspace_id: Uuid, id: &BlobId) -> Result<BlobInfo, BlobError> {
        self.store(workspace_id)?
            .stat_blob(workspace_id, id)
            .await
            .map_err(storage)?
            .ok_or_else(|| BlobError::NotFound(id.clone()))
    }

    /// Delete the blobs of every workspace that were put before
    /// `put_before` and that no version of a document of their workspace
    /// names (a blob put and not yet named is kept until then). A
    /// memory instance without a blob store sweeps nothing.
    ///
    /// # Errors
    /// The document kinds could not be read; the store failed.
    pub async fn sweep(
        &self,
        put_before: DateTime<Utc>,
        kinds: &dyn DocumentKinds,
    ) -> Result<Swept, BlobError> {
        let Some(store) = &self.store else {
            return Ok(Swept::default());
        };
        let mut swept = Swept::default();
        for workspace in self.workspaces.list_workspaces() {
            let Some(named) = self.documents.named_blobs(workspace.id, kinds).await? else {
                swept.skipped += 1;
                continue;
            };
            for kept in store.list_blobs(workspace.id).await.map_err(storage)? {
                if kept.put_at < put_before
                    && !named.contains(&kept.id)
                    && store
                        .delete_blob(workspace.id, &kept.id)
                        .await
                        .map_err(storage)?
                {
                    swept.deleted += 1;
                }
            }
        }
        Ok(swept)
    }

    fn store(&self, workspace_id: Uuid) -> Result<&dyn BlobStore, BlobError> {
        if self.workspaces.get_workspace(&workspace_id).is_none() {
            return Err(BlobError::WorkspaceNotFound(workspace_id));
        }
        self.store.as_deref().ok_or(BlobError::Unconfigured)
    }
}

#[cfg(test)]
mod tests;
