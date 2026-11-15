// Copyright (c) 2026 Julius ML
// MIT License

//! Response shapes of `manage_document`: a document with its data as the
//! JSON of its kind's message.

use chrono::{DateTime, Utc};
use post_cortex_core::core::document::{Document, DocumentKind};
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

/// One version of a document, its data as JSON.
#[derive(Debug, Serialize)]
pub struct DocumentJson {
    /// The workspace it lives in.
    pub workspace_id: Uuid,
    /// Its id.
    pub id: Uuid,
    /// Its kind.
    pub kind: DocumentKind,
    /// Its version.
    pub version: u64,
    /// Its data.
    pub data: Map<String, Value>,
    /// Its title.
    pub title: String,
    /// Its text.
    pub text: String,
    /// When this version was written.
    pub updated_at: DateTime<Utc>,
    /// Who wrote this version, when known.
    pub author: Option<String>,
}

impl DocumentJson {
    /// `document` with `data`, the JSON of its data.
    pub fn new(document: Document, data: Map<String, Value>) -> Self {
        Self {
            workspace_id: document.workspace_id,
            id: document.id,
            kind: document.kind,
            version: document.version,
            data,
            title: document.title,
            text: document.text,
            updated_at: document.updated_at,
            author: document.author,
        }
    }
}

/// A put: whether it was written, and the version written or the latest.
#[derive(Debug, Serialize)]
pub struct DocumentWritten {
    /// Whether the version was written (false: a conflict).
    pub written: bool,
    /// The version written; on a conflict the latest (none: no document).
    pub document: Option<DocumentJson>,
}

/// One version of a document.
#[derive(Debug, Serialize)]
pub struct DocumentVersion {
    /// The version.
    pub document: DocumentJson,
}

/// Versions of a document, newest first.
#[derive(Debug, Serialize)]
pub struct DocumentHistory {
    /// The versions.
    pub versions: Vec<DocumentJson>,
}

/// A page of documents.
#[derive(Debug, Serialize)]
pub struct DocumentList {
    /// The latest version of each, the most recently changed first.
    pub documents: Vec<DocumentJson>,
    /// Documents matching over all pages.
    pub total: usize,
    /// Where the next page starts; none on the last.
    pub next_cursor: Option<String>,
}

/// A deleted document.
#[derive(Debug, Serialize)]
pub struct DocumentDeleted {
    /// The document.
    pub document_id: String,
}
