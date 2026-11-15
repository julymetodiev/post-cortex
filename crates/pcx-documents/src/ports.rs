// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a module needs of its documents in one workspace (a kind is its
//! full name, `<module id>/<name>`), and of new sessions there: memory's
//! `pcx.v1` documents, their data a `google.protobuf.Any` of the kind's
//! message (W23).

use async_trait::async_trait;
use pcx_module_api::ModuleError;
use post_cortex_proto::pb::{Document, DocumentFilter, PutDocumentResponse};

/// A page of documents, the most recently changed first, and where the
/// next starts. A whole kind is read through [`crate::Typed::all`] or
/// [`crate::Typed::scan`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    /// The page.
    pub documents: Vec<Document>,
    /// The cursor of the next page; none on the last.
    pub next: Option<String>,
}

/// A module's documents in one workspace.
#[async_trait]
pub trait Documents: Send + Sync {
    /// The latest version of document `id` of `kind`; none when there is
    /// none.
    ///
    /// # Errors
    /// Memory failed.
    async fn get(&self, kind: &str, id: &str) -> Result<Option<Document>, ModuleError>;

    /// Version `version` of document `id` of `kind`; none when there is
    /// none.
    ///
    /// # Errors
    /// Memory failed.
    async fn version(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<Document>, ModuleError>;

    /// Write the next version of `id` (none: a new document) of `kind`, if
    /// its latest version is `expected` (0: new): the version written, or
    /// on a conflict the latest one.
    ///
    /// # Errors
    /// Memory refused the data, or failed.
    async fn put(
        &self,
        kind: &str,
        id: Option<&str>,
        expected: u64,
        data: prost_types::Any,
        text: &str,
    ) -> Result<PutDocumentResponse, ModuleError>;

    /// A page of at most `limit` of the documents of `kind` that `filter`
    /// keeps (none: all), the most recently changed first, after `cursor`
    /// (none: the first page). A page may hold fewer than `limit` while
    /// more follow (docs/memory.md, document keys and pages).
    ///
    /// # Errors
    /// Memory refused the filter, or failed.
    async fn page(
        &self,
        kind: &str,
        filter: Option<DocumentFilter>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page, ModuleError>;

    /// Delete document `id` of `kind` with its history: whether there was
    /// one.
    ///
    /// # Errors
    /// Memory failed.
    async fn delete(&self, kind: &str, id: &str) -> Result<bool, ModuleError>;
}

/// A module's new sessions in its workspace.
#[async_trait]
pub trait Sessions: Send + Sync {
    /// A new session named `name`; its id.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn create(&self, name: &str) -> Result<String, ModuleError>;
}
