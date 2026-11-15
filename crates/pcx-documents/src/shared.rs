// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`Documents`] of a store that is the organisation's: the one workspace a
//! module's settings name, whoever calls and from whichever workspace
//! (`docs/connect.md` section 4a, a module's own store). Every caller reads
//! it, and the module keeps its own records there (the log of what a call
//! did), without the caller's reach: the reach holds a person to their
//! workspaces' memory, not the module to its store. A caller's change of
//! any other kind is the caller's write: it needs the store's workspace in
//! the caller's reach, which memory holds it to as well.

use async_trait::async_trait;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::pb::{Document, DocumentFilter, PutDocumentResponse};

use crate::{Documents, HostDocuments, Page};

/// A module's documents in the organisation's store `workspace`, for one
/// call.
pub struct SharedDocuments<'a> {
    channel: GrpcChannel,
    caller: &'a Scope,
    /// The caller's scope without its reach.
    module: Scope,
    workspace: &'a str,
    /// The kinds the module itself writes for a call.
    records: Vec<String>,
}

impl<'a> SharedDocuments<'a> {
    /// The documents over the module's host `channel`, for a call in
    /// `caller`, in `workspace`; `records` are the kinds (full names) the
    /// module writes itself for a call, its log.
    #[must_use]
    pub fn new(
        channel: GrpcChannel,
        caller: &'a Scope,
        workspace: &'a str,
        records: Vec<String>,
    ) -> Self {
        Self {
            channel,
            caller,
            module: caller.in_own_store(),
            workspace,
            records,
        }
    }

    /// The documents as the module reads them and writes its records.
    fn own(&self) -> HostDocuments<'_> {
        HostDocuments::new(self.channel.clone(), &self.module, self.workspace)
    }

    /// The documents as a write of `kind` goes: the module's for a record,
    /// else the caller's.
    ///
    /// # Errors
    /// `Denied`: a change by a caller who does not reach the store.
    fn written(&self, kind: &str) -> Result<HostDocuments<'_>, ModuleError> {
        if self.records.iter().any(|record| record == kind) {
            return Ok(self.own());
        }
        if !self.caller.reaches(self.workspace) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!(
                    "{kind} is the organisation's, kept in a workspace you cannot use: you may \
                     read and use it; a person who can use that workspace changes it"
                ),
            ));
        }
        Ok(HostDocuments::new(
            self.channel.clone(),
            self.caller,
            self.workspace,
        ))
    }
}

#[async_trait]
impl Documents for SharedDocuments<'_> {
    async fn get(&self, kind: &str, id: &str) -> Result<Option<Document>, ModuleError> {
        self.own().get(kind, id).await
    }

    async fn version(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<Document>, ModuleError> {
        self.own().version(kind, id, version).await
    }

    async fn put(
        &self,
        kind: &str,
        id: Option<&str>,
        expected: u64,
        data: prost_types::Any,
        text: &str,
    ) -> Result<PutDocumentResponse, ModuleError> {
        self.written(kind)?
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
        self.own().page(kind, filter, limit, cursor).await
    }

    async fn delete(&self, kind: &str, id: &str) -> Result<bool, ModuleError> {
        self.written(kind)?.delete(kind, id).await
    }
}
