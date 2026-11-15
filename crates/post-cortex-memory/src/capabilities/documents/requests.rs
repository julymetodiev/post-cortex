// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the Documents capability is asked and answers: a version to write,
//! a listing with its filter, and their outcomes.

use post_cortex_core::core::document::{Document, DocumentData, DocumentKind};
use uuid::Uuid;

use crate::graph_views::detail::PageCursor;

/// A new version of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct PutDocument {
    /// Its workspace.
    pub workspace_id: Uuid,
    /// The document; none for a new one (with `expected_version` 0).
    pub id: Option<Uuid>,
    /// Its kind.
    pub kind: DocumentKind,
    /// Its data: a message of its kind's type (JSON from the MCP tool is
    /// read into it).
    pub data: DocumentData,
    /// Its text.
    pub text: String,
    /// Who writes this version.
    pub author: Option<String>,
    /// The version the writer read (0: none).
    pub expected_version: u64,
}

/// The answer of [`crate::capabilities::Documents::put`]: a conflict is a normal answer.
#[derive(Debug, Clone, PartialEq)]
pub struct PutOutcome {
    /// Whether the version was written.
    pub written: bool,
    /// The version written, or on a conflict the latest one.
    pub document: Option<Document>,
}

/// A listing of the documents of one kind in a workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct ListDocuments {
    /// The workspace.
    pub workspace_id: Uuid,
    /// The kind.
    pub kind: DocumentKind,
    /// Only documents whose data matches; none: all of them.
    pub filter: Option<DocumentFilter>,
    /// Where the page starts.
    pub after: Option<PageCursor<Uuid>>,
    /// At most this many.
    pub limit: usize,
}

/// Documents whose data has some top-level fields equal to an example's.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentFilter {
    /// Data of the kind's type with the values to match.
    pub example: DocumentData,
    /// The fields of it that must be equal, by name.
    pub fields: Vec<String>,
}

/// A page of documents, the most recently changed first.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentPage {
    /// The latest version of each.
    pub documents: Vec<Document>,
    /// Documents matching over all pages.
    pub total: usize,
    /// Where the next page starts; `None` on the last.
    pub next: Option<PageCursor<Uuid>>,
}
