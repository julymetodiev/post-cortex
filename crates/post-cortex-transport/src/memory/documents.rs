// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC documents of modules (W23): their data as `google.protobuf.Any`
//! of the kind's message; the writer is the module the host names in the
//! call's metadata.

use post_cortex_core::core::document::{Document as Stored, DocumentData, DocumentKind};
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::{DocumentFilter, ListDocuments, PutDocument, Writer};
use post_cortex_memory::graph_views::detail::PageCursor;
use post_cortex_memory::graph_views::{DEFAULT_LIMIT, MAX_LIMIT};
use post_cortex_ports::DocumentKinds;
use post_cortex_proto::pb::*;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::grpc::status;
use crate::memory::helpers::parse_uuid;
use crate::views::response::timestamp;

/// The prefix of an `Any`'s type URL.
const TYPE_PREFIX: &str = "type.googleapis.com/";

fn data(any: Option<prost_types::Any>) -> Result<DocumentData, Status> {
    let any = any.ok_or_else(|| Status::invalid_argument("the request has no data"))?;
    let type_name = any
        .type_url
        .rsplit_once('/')
        .map_or(any.type_url.as_str(), |(_, name)| name);
    Ok(DocumentData::Message {
        type_name: type_name.to_string(),
        bytes: any.value,
    })
}

fn any(data: DocumentData) -> Result<prost_types::Any, Status> {
    match data {
        DocumentData::Message { type_name, bytes } => Ok(prost_types::Any {
            type_url: format!("{TYPE_PREFIX}{type_name}"),
            value: bytes,
        }),
        DocumentData::Json(_) => Err(Status::failed_precondition(
            "the document holds data written before W23; migrate the documents first",
        )),
    }
}

fn document(d: Stored) -> Result<Document, Status> {
    Ok(Document {
        workspace_id: d.workspace_id.to_string(),
        id: d.id.to_string(),
        kind: d.kind.to_string(),
        version: d.version,
        data: Some(any(d.data)?),
        title: d.title,
        text: d.text,
        updated_at: Some(timestamp(d.updated_at)),
        author: d.author,
    })
}

fn documents(ds: Vec<Stored>) -> Result<Vec<Document>, Status> {
    ds.into_iter().map(document).collect()
}

fn kind(raw: &str) -> Result<DocumentKind, Status> {
    raw.parse()
        .map_err(|e| Status::invalid_argument(format!("kind: {e}")))
}

/// A limit on the wire: 0 is not given.
fn limit(raw: u32) -> usize {
    match raw {
        0 => DEFAULT_LIMIT,
        n => (n as usize).min(MAX_LIMIT),
    }
}

pub async fn put(
    memory: &Memory,
    kinds: &dyn DocumentKinds,
    request: Request<PutDocumentRequest>,
) -> Result<Response<PutDocumentResponse>, Status> {
    let scope = pcx_module_api::Scope::of(&request)?;
    let reach = reach_of(&scope)?;
    let req = request.into_inner();
    let id = match req.document_id.as_str() {
        "" => None,
        raw => Some(parse_uuid(raw)?),
    };
    let put = PutDocument {
        workspace_id: parse_uuid(&req.workspace_id)?,
        id,
        kind: kind(&req.kind)?,
        data: data(req.data)?,
        text: req.text,
        author: req.author.or(scope.author),
        expected_version: req.expected_version,
    };
    let writer = Writer {
        module: scope.caller_module.as_deref(),
        kinds,
        reach: reach.as_deref(),
    };
    let outcome = memory.documents.put(put, writer).await.map_err(status)?;
    Ok(Response::new(PutDocumentResponse {
        written: outcome.written,
        document: outcome.document.map(document).transpose()?,
    }))
}

pub async fn get(
    memory: &Memory,
    request: Request<GetDocumentRequest>,
) -> Result<Response<Document>, Status> {
    let req = request.into_inner();
    let found = memory
        .documents
        .get(
            parse_uuid(&req.workspace_id)?,
            parse_uuid(&req.document_id)?,
            req.version,
        )
        .await
        .map_err(status)?;
    Ok(Response::new(document(found)?))
}

pub async fn history(
    memory: &Memory,
    request: Request<GetDocumentHistoryRequest>,
) -> Result<Response<DocumentHistory>, Status> {
    let req = request.into_inner();
    let versions = memory
        .documents
        .history(
            parse_uuid(&req.workspace_id)?,
            parse_uuid(&req.document_id)?,
            limit(req.limit),
        )
        .await
        .map_err(status)?;
    Ok(Response::new(DocumentHistory {
        versions: documents(versions)?,
    }))
}

pub async fn list(
    memory: &Memory,
    kinds: &dyn DocumentKinds,
    request: Request<ListDocumentsRequest>,
) -> Result<Response<DocumentPage>, Status> {
    let req = request.into_inner();
    let after = match req.cursor.as_str() {
        "" => None,
        raw => Some(raw.parse::<PageCursor<Uuid>>().map_err(|e| {
            Status::invalid_argument(format!("cursor: {e}: pass the previous page's next_cursor"))
        })?),
    };
    let filter = req
        .filter
        .map(|f| {
            Ok::<_, Status>(DocumentFilter {
                example: data(f.example)?,
                fields: f.fields,
            })
        })
        .transpose()?;
    let page = memory
        .documents
        .list(
            ListDocuments {
                workspace_id: parse_uuid(&req.workspace_id)?,
                kind: kind(&req.kind)?,
                filter,
                after,
                limit: limit(req.limit),
            },
            kinds,
        )
        .await
        .map_err(status)?;
    Ok(Response::new(DocumentPage {
        documents: documents(page.documents)?,
        total: page.total as u64,
        next_cursor: page.next.map(|c| c.to_string()).unwrap_or_default(),
    }))
}

pub async fn delete(
    memory: &Memory,
    kinds: &dyn DocumentKinds,
    request: Request<DeleteDocumentRequest>,
) -> Result<Response<DeleteDocumentResponse>, Status> {
    let scope = pcx_module_api::Scope::of(&request)?;
    let req = request.into_inner();
    let reach = reach_of(&scope)?;
    let writer = Writer {
        module: scope.caller_module.as_deref(),
        kinds,
        reach: reach.as_deref(),
    };
    memory
        .documents
        .delete(
            parse_uuid(&req.workspace_id)?,
            parse_uuid(&req.document_id)?,
            writer,
        )
        .await
        .map_err(status)?;
    Ok(Response::new(DeleteDocumentResponse {}))
}

/// The reach of the call `scope` is of, for its writes.
///
/// # Errors
/// `InvalidArgument` for a reach that names no workspace id.
fn reach_of(scope: &pcx_module_api::Scope) -> Result<Option<Vec<uuid::Uuid>>, Status> {
    scope
        .reach
        .as_deref()
        .map(post_cortex_memory::reach::workspaces)
        .transpose()
        .map_err(|e| Status::invalid_argument(e.to_string()))
}
