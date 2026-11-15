// Copyright (c) 2026 Julius ML
// MIT License

//! Tool `manage_document`: the documents modules keep in memory
//! (ARCHITECTURE section 7, modules together). Writes take the caller the
//! host names and the port to the running modules' kinds.

use post_cortex_core::core::document::{Document, DocumentData, DocumentKind};
use serde_json::Value;
use uuid::Uuid;

use crate::Memory;
use crate::capabilities::{DocumentFilter, ListDocuments, PutDocument, Writer};
use crate::graph_views::detail::PageCursor;
use crate::graph_views::{DEFAULT_LIMIT, MAX_LIMIT};
use crate::tools::args::ManageDocumentRequest;
use crate::tools::handlers::{Caller, Outcome};
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::responses::{
    DocumentDeleted, DocumentHistory, DocumentJson, DocumentList, DocumentVersion, DocumentWritten,
};
use crate::tools::{Failure, check, reply, wire};

const TOOL: &str = "manage_document";

const ACTION_HINT: &str = "Valid actions: put (kind, data, text, expected_version; document_id for a change), get (document_id, optional version), history (document_id, optional limit), list (kind, optional fields, limit, cursor), delete (document_id)";

const HINTS: &[FieldHint] = &[FieldHint {
    field: "action",
    expected: "one of: put, get, history, list, delete",
    hint: ACTION_HINT,
}];

pub async fn handle(memory: &Memory, params: Value, caller: &Caller<'_>) -> Outcome {
    let req: ManageDocumentRequest = arguments(params, HINTS)?;
    let action = crate::tools::handlers::action(TOOL, &req.action, ACTION_HINT)?;
    let workspace_id = check::workspace_id(&req.workspace_id)?;
    let docs = &memory.documents;
    match action.as_str() {
        "put" => {
            let writer = Writer {
                module: caller.module,
                kinds: caller.kinds,
                reach: caller.reach.as_deref(),
            };
            let expected_version = req.expected_version.ok_or_else(|| {
                Failure::invalid(
                    "put needs expected_version: the version you read (0 for a new document)",
                )
                .parameter("expected_version")
            })?;
            let put = PutDocument {
                workspace_id,
                id: req.document_id.as_deref().map(document_id).transpose()?,
                kind: kind(req.kind.as_deref())?,
                data: DocumentData::Json(req.data.ok_or_else(|| {
                    Failure::invalid("put needs data: the JSON of its kind's message")
                        .parameter("data")
                })?),
                text: req.text.unwrap_or_default(),
                author: crate::tools::handlers::author(
                    req.author.as_deref(),
                    caller.author.clone(),
                ),
                expected_version,
            };
            let outcome = docs.put(put, writer).await.map_err(Failure::from)?;
            let message = match (&outcome.written, &outcome.document) {
                (true, Some(d)) => format!("Document {} saved as version {}", d.id, d.version),
                (_, Some(d)) => format!(
                    "Not saved: the document is at version {} now, not {expected_version}. Decide again from the version below and retry with expected_version {}",
                    d.version, d.version
                ),
                (_, None) => "Not saved: there is no such document".to_string(),
            };
            let document = match outcome.document {
                Some(document) => json(memory, caller, vec![document]).await?.pop(),
                None => None,
            };
            reply::with(
                message,
                &DocumentWritten {
                    written: outcome.written,
                    document,
                },
            )
        }
        "get" => {
            let id = required_id(req.document_id.as_deref())?;
            let document = docs
                .get(workspace_id, id, req.version)
                .await
                .map_err(Failure::from)?;
            let message = format!("Document {id} version {}", document.version);
            let Some(document) = json(memory, caller, vec![document]).await?.pop() else {
                return Err(Failure::internal("a document read maps to no JSON"));
            };
            reply::with(message, &DocumentVersion { document })
        }
        "history" => {
            let id = required_id(req.document_id.as_deref())?;
            let limit = limit(req.limit)?;
            let versions = docs
                .history(workspace_id, id, limit)
                .await
                .map_err(Failure::from)?;
            let message = format!("{} versions of document {id}", versions.len());
            let versions = json(memory, caller, versions).await?;
            reply::with(message, &DocumentHistory { versions })
        }
        "list" => {
            let after = req
                .cursor
                .as_deref()
                .map(|raw| {
                    raw.parse::<PageCursor<Uuid>>().map_err(|e| {
                        Failure::invalid(format!("{e}: pass the next_cursor of the previous page"))
                            .parameter("cursor")
                    })
                })
                .transpose()?;
            // Fields to match: their names, the values those of an example.
            let filter = req
                .fields
                .filter(|f| !f.is_empty())
                .map(|fields| DocumentFilter {
                    fields: fields.keys().cloned().collect(),
                    example: DocumentData::Json(fields),
                });
            let page = docs
                .list(
                    ListDocuments {
                        workspace_id,
                        kind: kind(req.kind.as_deref())?,
                        filter,
                        after,
                        limit: limit(req.limit)?,
                    },
                    caller.kinds,
                )
                .await
                .map_err(Failure::from)?;
            let message = format!("{} of {} documents", page.documents.len(), page.total);
            reply::with(
                message,
                &DocumentList {
                    documents: json(memory, caller, page.documents).await?,
                    total: page.total,
                    next_cursor: page.next.map(|c| c.to_string()),
                },
            )
        }
        "delete" => {
            let id = required_id(req.document_id.as_deref())?;
            let writer = Writer {
                module: caller.module,
                kinds: caller.kinds,
                reach: caller.reach.as_deref(),
            };
            docs.delete(workspace_id, id, writer)
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!("Deleted document {id}"),
                &DocumentDeleted {
                    document_id: id.to_string(),
                },
            )
        }
        other => Err(crate::tools::handlers::unhandled(TOOL, other)),
    }
}

/// `documents` with their data as JSON.
async fn json(
    memory: &Memory,
    caller: &Caller<'_>,
    documents: Vec<Document>,
) -> Result<Vec<DocumentJson>, Failure> {
    let data = memory
        .documents
        .json_data(&documents, caller.kinds)
        .await
        .map_err(Failure::from)?;
    Ok(documents
        .into_iter()
        .zip(data)
        .map(|(document, data)| DocumentJson::new(document, data))
        .collect())
}

fn kind(raw: Option<&str>) -> Result<DocumentKind, Failure> {
    let raw = raw.ok_or_else(|| {
        Failure::invalid("this action needs kind: <module>/<name>, e.g. board/task")
            .parameter("kind")
    })?;
    raw.parse::<DocumentKind>().map_err(|e| {
        Failure::invalid(e.to_string())
            .parameter("kind")
            .expected("<module>/<name>, e.g. board/task")
    })
}

fn document_id(raw: &str) -> Result<Uuid, Failure> {
    wire::uuid("document_id", raw)
}

fn required_id(raw: Option<&str>) -> Result<Uuid, Failure> {
    let raw = raw.ok_or_else(|| {
        Failure::invalid("this action needs document_id").parameter("document_id")
    })?;
    document_id(raw)
}

fn limit(raw: Option<u32>) -> Result<usize, Failure> {
    let raw = raw
        .map(usize::try_from)
        .transpose()
        .map_err(|e| Failure::invalid(format!("limit: {e}")).parameter("limit"))?;
    wire::limit("limit", raw, DEFAULT_LIMIT, MAX_LIMIT)
}
