// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx.v1.Blobs` gRPC service of memory (W24): an image's bytes in,
//! in chunks, and what it is out; a blob's bytes out in chunks. The call's
//! workspace is its scope's, else the one the request names.

use std::pin::Pin;

use futures::{Stream, StreamExt};
use pcx_module_api::Scope;
use post_cortex_core::core::blob::MAX_BLOB_BYTES;
use post_cortex_proto::pb::blobs_server::{Blobs, BlobsServer};
use post_cortex_proto::pb::{BlobChunk, BlobInfo, BlobRequest, PutBlobChunk};
use tonic::{Request, Response, Status, Streaming};

use crate::memory::PcxGrpcService;

/// Bytes of a blob per chunk sent.
const CHUNK_BYTES: usize = 256 * 1024;

/// The chunks of a blob.
pub type BlobStream = Pin<Box<dyn Stream<Item = Result<BlobChunk, Status>> + Send>>;

impl PcxGrpcService {
    /// The tonic server of its `pcx.v1.Blobs` service.
    pub fn into_blobs_server(self) -> BlobsServer<Self> {
        BlobsServer::new(self)
    }
}

#[tonic::async_trait]
impl Blobs for PcxGrpcService {
    async fn put_blob(
        &self,
        request: Request<Streaming<PutBlobChunk>>,
    ) -> Result<Response<BlobInfo>, Status> {
        let scope = Scope::of(&request)?;
        let mut chunks = request.into_inner();
        let mut named = None;
        let mut declared = None;
        let mut bytes = Vec::new();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk?;
            named.get_or_insert(chunk.workspace_id);
            declared.get_or_insert(chunk.media_type);
            // One byte over the limit is enough for the capability to refuse.
            if bytes.len() <= MAX_BLOB_BYTES {
                bytes.extend_from_slice(&chunk.data);
            }
        }
        let workspace = scope.workspace(named.as_deref().unwrap_or_default())?;
        crate::blobs::put(
            self.memory(),
            &workspace,
            bytes,
            declared.as_deref().unwrap_or_default(),
        )
        .await
        .map(Response::new)
    }

    type GetBlobStream = BlobStream;

    async fn get_blob(
        &self,
        request: Request<BlobRequest>,
    ) -> Result<Response<BlobStream>, Status> {
        let scope = Scope::of(&request)?;
        let req = request.into_inner();
        let workspace = scope.workspace(&req.workspace_id)?;
        let blob = crate::blobs::get(self.memory(), &workspace, &req.sha256).await?;
        let first = crate::blobs::info(&blob.info);
        let chunks: Vec<Result<BlobChunk, Status>> = blob
            .bytes
            .chunks(CHUNK_BYTES)
            .enumerate()
            .map(|(i, data)| {
                Ok(BlobChunk {
                    info: (i == 0).then(|| first.clone()),
                    data: data.to_vec(),
                })
            })
            .collect();
        Ok(Response::new(Box::pin(futures::stream::iter(chunks))))
    }

    async fn stat_blob(&self, request: Request<BlobRequest>) -> Result<Response<BlobInfo>, Status> {
        let scope = Scope::of(&request)?;
        let req = request.into_inner();
        let workspace = scope.workspace(&req.workspace_id)?;
        crate::blobs::stat(self.memory(), &workspace, &req.sha256)
            .await
            .map(Response::new)
    }
}
