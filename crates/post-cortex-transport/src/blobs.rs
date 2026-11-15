// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Blobs over `pcx.v1` (`PutBlob`, `GetBlob`, `StatBlob`; W24), the one
//! mapping both the gRPC service and the browser transfer
//! ([`crate::browser::transfer::blobs`]) use. The checks are the Blobs
//! capability's; these translate the wire types.

use post_cortex_core::core::blob::{BlobId, BlobInfo as Info};
use post_cortex_memory::Memory;
use post_cortex_ports::Blob;
use post_cortex_proto::pb;
use tonic::Status;

use crate::grpc::status;
use crate::views::request::uuid;

/// What a blob is, on the wire.
#[must_use]
pub fn info(blob: &Info) -> pb::BlobInfo {
    pb::BlobInfo {
        sha256: blob.id.to_string(),
        size: blob.size,
        media_type: blob.image.media_type.as_str().to_string(),
        width: blob.image.width,
        height: blob.image.height,
    }
}

fn id(raw: &str) -> Result<BlobId, Status> {
    raw.parse()
        .map_err(|e| Status::invalid_argument(format!("sha256: {e}")))
}

/// Keep `bytes` in workspace `workspace_id`.
///
/// # Errors
/// A workspace id that is none; what the capability refuses.
pub async fn put(
    memory: &Memory,
    workspace_id: &str,
    bytes: Vec<u8>,
    declared: &str,
) -> Result<pb::BlobInfo, Status> {
    let put = memory
        .blobs
        .put(uuid(workspace_id, "workspace_id")?, bytes, declared)
        .await
        .map_err(status)?;
    Ok(info(&put))
}

/// Blob `sha256` of workspace `workspace_id`.
///
/// # Errors
/// An id that is none; no such blob; the capability failed.
pub async fn get(memory: &Memory, workspace_id: &str, sha256: &str) -> Result<Blob, Status> {
    memory
        .blobs
        .get(uuid(workspace_id, "workspace_id")?, &id(sha256)?)
        .await
        .map_err(status)
}

/// What blob `sha256` of workspace `workspace_id` is.
///
/// # Errors
/// An id that is none; no such blob; the capability failed.
pub async fn stat(
    memory: &Memory,
    workspace_id: &str,
    sha256: &str,
) -> Result<pb::BlobInfo, Status> {
    let found = memory
        .blobs
        .stat(uuid(workspace_id, "workspace_id")?, &id(sha256)?)
        .await
        .map_err(status)?;
    Ok(info(&found))
}
