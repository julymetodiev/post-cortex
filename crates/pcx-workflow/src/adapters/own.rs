// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Memory's documents of the module's workspace, held past one borrow: a
//! call's (a workflow written by its author), or the module's own for its
//! runs that outlive any call (docs/modules/workflow.md, the engine), each
//! of those made in a fresh scope written by the module.

use std::time::Duration;

use async_trait::async_trait;
use pcx_documents::{Documents, HostDocuments, Page};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::pb::{Document, DocumentFilter, PutDocumentResponse};
use tokio_util::sync::CancellationToken;

use crate::domain::MODULE_ID;

/// The deadline of each of the module's own calls to memory.
const DEADLINE: Duration = Duration::from_secs(60);

/// The documents of the module's workspace.
pub struct OwnDocuments {
    channel: GrpcChannel,
    workspace: String,
    /// The call's scope; none: the module's own, fresh each time.
    call: Option<Scope>,
}

impl OwnDocuments {
    /// The module's own documents of `workspace` over its `channel`.
    pub fn new(channel: GrpcChannel, workspace: String) -> Self {
        Self {
            channel,
            workspace,
            call: None,
        }
    }

    /// The documents of `workspace` for a call in `scope`, written by its
    /// author.
    pub fn of_call(channel: GrpcChannel, workspace: String, scope: &Scope) -> Self {
        Self {
            channel,
            workspace,
            call: Some(scope.clone()),
        }
    }

    fn scope(&self) -> Scope {
        if let Some(call) = &self.call {
            return call.clone();
        }
        Scope::own(
            MODULE_ID,
            &self.workspace,
            DEADLINE,
            CancellationToken::new(),
        )
    }
}

#[async_trait]
impl Documents for OwnDocuments {
    async fn get(&self, kind: &str, id: &str) -> Result<Option<Document>, ModuleError> {
        let scope = self.scope();
        HostDocuments::new(self.channel.clone(), &scope, &self.workspace)
            .get(kind, id)
            .await
    }

    async fn version(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<Document>, ModuleError> {
        let scope = self.scope();
        HostDocuments::new(self.channel.clone(), &scope, &self.workspace)
            .version(kind, id, version)
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
        let scope = self.scope();
        HostDocuments::new(self.channel.clone(), &scope, &self.workspace)
            .put(kind, id, expected, data, text)
            .await
    }

    async fn page(
        &self,
        kind: &str,
        filter: Option<DocumentFilter>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page, ModuleError> {
        let scope = self.scope();
        HostDocuments::new(self.channel.clone(), &scope, &self.workspace)
            .page(kind, filter, limit, cursor)
            .await
    }

    async fn delete(&self, kind: &str, id: &str) -> Result<bool, ModuleError> {
        let scope = self.scope();
        HostDocuments::new(self.channel.clone(), &scope, &self.workspace)
            .delete(kind, id)
            .await
    }
}
