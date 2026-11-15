// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The document port.

use crate::error::Result;
use async_trait::async_trait;
use post_cortex_core::core::document::{
    Document, DocumentCursor, DocumentDraft, DocumentKey, DocumentKind,
};
use uuid::Uuid;

/// Outcome of [`DocumentStore::put_document`].
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentWrite {
    /// The draft was stored as this new version.
    Written(Document),
    /// The latest stored version was not the expected one, or is of
    /// another kind; nothing was written. Carries the latest version
    /// (`None`: no such document).
    Conflict(Option<Document>),
}

/// A page of the latest versions of one kind (docs/memory.md, document
/// keys and pages).
#[derive(Debug, Clone, Copy)]
pub struct LatestQuery<'a> {
    /// The workspace.
    pub workspace_id: Uuid,
    /// The kind.
    pub kind: &'a DocumentKind,
    /// Only documents that have every one of these keys; none: all.
    pub keys: &'a [DocumentKey],
    /// Where the page starts: after this document; none: the first page.
    pub after: Option<DocumentCursor>,
    /// At most this many.
    pub limit: usize,
}

/// What [`DocumentStore::latest_page`] answers.
#[derive(Debug, Clone, PartialEq)]
pub struct LatestPage {
    /// The latest versions, the most recently changed first.
    pub documents: Vec<Document>,
    /// Documents matching over all pages.
    pub total: usize,
    /// Where the next page starts; `None` on the last.
    pub next: Option<DocumentCursor>,
}

/// Storage for the documents of modules (see
/// [`post_cortex_core::core::document`]). A document is keyed by its
/// workspace and its id; every write is a new version and every version is
/// kept until the document or its workspace is deleted. The latest version
/// of each carries the keys it was written with, and is found by them, in
/// the order of the most recent change (time descending, then id).
#[async_trait]
pub trait DocumentStore: Send + Sync {
    /// The latest version of document `id` of `workspace_id`, or its
    /// `version` when one is named.
    async fn get_document(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        version: Option<u64>,
    ) -> Result<Option<Document>>;

    /// Store `draft` as the next version of its document, with its keys,
    /// if the latest version is `expected_version` (0: no document yet)
    /// and of the draft's kind.
    async fn put_document(
        &self,
        draft: DocumentDraft,
        expected_version: u64,
    ) -> Result<DocumentWrite>;

    /// Up to `limit` versions of document `id` of `workspace_id`, newest
    /// first.
    async fn document_history(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        limit: usize,
    ) -> Result<Vec<Document>>;

    /// The latest version of every document of `workspace_id`, of `kind`
    /// when one is named, in no particular order: for what reads them all
    /// (an export, the blob sweep).
    async fn latest_documents(
        &self,
        workspace_id: Uuid,
        kind: Option<&DocumentKind>,
    ) -> Result<Vec<Document>>;

    /// A page of the latest versions `query` names, reading only the page
    /// and the keys that count it.
    async fn latest_page(&self, query: LatestQuery<'_>) -> Result<LatestPage>;

    /// The latest versions of `workspace_id` stored without keys.
    async fn unkeyed_documents(&self, workspace_id: Uuid) -> Result<Vec<Document>>;

    /// Give the latest version of document `id` of `workspace_id` the
    /// keys `keys`, if it is still `version`: whether it was.
    async fn set_document_keys(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        version: u64,
        keys: Vec<DocumentKey>,
    ) -> Result<bool>;

    /// Delete document `id` of `workspace_id` with every version. Whether
    /// it existed.
    async fn delete_document(&self, workspace_id: Uuid, id: Uuid) -> Result<bool>;

    /// Store `versions` of document `id` of `workspace_id` as they are,
    /// with their version numbers, times and authors (an import), after
    /// deleting the stored versions when `replace`. The highest version
    /// stored becomes the latest, with `keys` (none: unkeyed).
    async fn restore_document_versions(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        versions: Vec<Document>,
        replace: bool,
        keys: Option<Vec<DocumentKey>>,
    ) -> Result<()>;
}
