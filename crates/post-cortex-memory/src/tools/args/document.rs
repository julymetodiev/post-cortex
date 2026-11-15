// Copyright (c) 2026 Julius ML
// MIT License

//! Arguments of the `manage_document` tool. The schemars attributes are the
//! tool's published input schema.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

/// Request payload for the `manage_document` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageDocumentRequest {
    /// Document action (`put`, `get`, `history`, `list`, `delete`).
    #[schemars(description = r#"Action on the documents modules keep in a workspace.

Valid values:
- put: Write the next version of a document (kind, data, text, expected_version; document_id for a change, none for a new one)
- get: The latest version of a document, or one version (document_id, optional version)
- history: Earlier versions of a document, newest first (document_id, optional limit)
- list: The documents of a kind, most recently changed first (kind, optional fields, limit, cursor)
- delete: Delete a document with every version (document_id)

Only the module that declares a kind (board/task) writes and deletes its documents; anyone who
reads the workspace reads them. put needs the version you read (0 for a new document); if someone
changed it since, nothing is written and the latest version comes back so you can decide again."#)]
    pub action: String,
    /// The workspace the documents live in.
    #[schemars(description = "Workspace ID (36-char UUID) the documents live in. Required.")]
    pub workspace_id: String,
    /// The document, for get, history, delete and a put that changes one.
    #[schemars(
        description = "Document ID (36-char UUID): for get, history, delete, and put of a change. A new document has none; memory gives it one."
    )]
    pub document_id: Option<String>,
    /// The kind, `<module>/<name>`, for put and list.
    #[schemars(description = "Document kind, <module>/<name> (board/task), for put and list.")]
    pub kind: Option<String>,
    /// The data of a put, checked against its kind's schema.
    #[schemars(description = "The document's data for put: an object its kind's schema takes.")]
    pub data: Option<Map<String, Value>>,
    /// The text of a put.
    #[schemars(description = "The document's text for put (at most 32 KiB); search finds it.")]
    pub text: Option<String>,
    /// The version a put changes (0: a new document).
    #[schemars(description = "For put: the version you read (0 for a new document).")]
    pub expected_version: Option<u64>,
    /// One version for get.
    #[schemars(description = "For get: this version instead of the latest.")]
    pub version: Option<u64>,
    /// Data fields a listed document has, with these values.
    #[schemars(
        description = "For list: top-level data fields and the values a listed document has (all must match), e.g. {\"column\": \"todo\"}."
    )]
    pub fields: Option<Map<String, Value>>,
    /// At most this many (list, history).
    #[schemars(description = "For list and history: at most this many (default 100, max 5000).")]
    pub limit: Option<u32>,
    /// Where a list page starts.
    #[schemars(description = "For list: the next_cursor of the previous page.")]
    pub cursor: Option<String>,
    /// Who writes, when not the caller's own name.
    #[schemars(description = "Who writes this version (optional; defaults to the caller).")]
    pub author: Option<String>,
}
