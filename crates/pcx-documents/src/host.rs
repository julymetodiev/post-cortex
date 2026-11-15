// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`Documents`] over memory's `pcx.v1` documents through a module's host
//! channel (W23), in the caller's scope: the host names the module as the
//! writer, the scope's author as the author.

use async_trait::async_trait;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use post_cortex_proto::pb::{
    DeleteDocumentRequest, Document, DocumentFilter, GetDocumentRequest, ListDocumentsRequest,
    PutDocumentRequest, PutDocumentResponse,
};
use tonic::Request;

use crate::{Documents, Page};

/// A module's documents in `workspace`, for one call.
pub struct HostDocuments<'a> {
    channel: GrpcChannel,
    scope: &'a Scope,
    workspace: &'a str,
}

impl<'a> HostDocuments<'a> {
    /// The documents over the module's host `channel`, in `scope`, in
    /// `workspace`.
    #[must_use]
    pub fn new(channel: GrpcChannel, scope: &'a Scope, workspace: &'a str) -> Self {
        Self {
            channel,
            scope,
            workspace,
        }
    }

    fn client(&self) -> PostCortexClient<GrpcChannel> {
        PostCortexClient::new(self.channel.clone())
    }

    /// `message` as a request in the call's scope.
    fn request<T>(&self, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        self.scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }

    /// The version `request` reads, if it is of `kind`.
    async fn read(
        &self,
        kind: &str,
        request: GetDocumentRequest,
    ) -> Result<Option<Document>, ModuleError> {
        match self.client().get_document(self.request(request)?).await {
            // An id of another kind is not this kind's document.
            Ok(found) => Ok(Some(found.into_inner()).filter(|d| d.kind == kind)),
            Err(status) => match ModuleError::from(status) {
                e if e.kind == ModuleErrorKind::NotFound => Ok(None),
                e => Err(e),
            },
        }
    }
}

#[async_trait]
impl Documents for HostDocuments<'_> {
    async fn get(&self, kind: &str, id: &str) -> Result<Option<Document>, ModuleError> {
        self.read(
            kind,
            GetDocumentRequest {
                workspace_id: self.workspace.to_string(),
                document_id: id.to_string(),
                version: None,
            },
        )
        .await
    }

    async fn version(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<Document>, ModuleError> {
        self.read(
            kind,
            GetDocumentRequest {
                workspace_id: self.workspace.to_string(),
                document_id: id.to_string(),
                version: Some(version),
            },
        )
        .await
    }

    async fn put(
        &self,
        kind: &str,
        id: Option<&str>,
        expected: u64,
        data: prost_types::Any,
        text: &str,
    ) -> Result<PutDocumentResponse, ModuleError> {
        let request = PutDocumentRequest {
            workspace_id: self.workspace.to_string(),
            document_id: id.unwrap_or_default().to_string(),
            kind: kind.to_string(),
            data: Some(data),
            text: text.to_string(),
            expected_version: expected,
            author: None,
        };
        Ok(self
            .client()
            .put_document(self.request(request)?)
            .await?
            .into_inner())
    }

    async fn page(
        &self,
        kind: &str,
        filter: Option<DocumentFilter>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page, ModuleError> {
        let request = ListDocumentsRequest {
            workspace_id: self.workspace.to_string(),
            kind: kind.to_string(),
            filter,
            limit,
            cursor: cursor.unwrap_or_default().to_string(),
        };
        let page = self
            .client()
            .list_documents(self.request(request)?)
            .await?
            .into_inner();
        Ok(Page {
            documents: page.documents,
            next: Some(page.next_cursor).filter(|next| !next.is_empty()),
        })
    }

    async fn delete(&self, kind: &str, id: &str) -> Result<bool, ModuleError> {
        // By id only in memory: first that it is of this kind.
        if self.get(kind, id).await?.is_none() {
            return Ok(false);
        }
        self.client()
            .delete_document(self.request(DeleteDocumentRequest {
                workspace_id: self.workspace.to_string(),
                document_id: id.to_string(),
            })?)
            .await?;
        Ok(true)
    }
}
