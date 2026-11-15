// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A request of any transport as the operation table sees it (ARCHITECTURE
//! section 6): its operation, the sessions and workspaces it names, the
//! project key it resolves, whether it asks for the reranker, and what it
//! counts. A gateway in front of many memory instances admits, limits and
//! counts calls from this, and holds no parsing of its own.
//!
//! Every surface is read into the JSON form of its request (an MCP call's
//! arguments, a gRPC message through its descriptor with the proto field
//! names, a REST request's path parameters, query and body), and the
//! table's fields are read from that (`json`).

use serde_json::Value;

mod grpc;
mod host;
mod rest;

pub use grpc::{
    answer_count as grpc_answer_count, answer_flag as grpc_answer_flag, changes as grpc_changes,
    grpc, without_rerank as grpc_without_rerank,
};
pub use host::{HostCall, host};
pub use post_cortex_memory::tools::operations::classify::{
    Amount, By, Classified, ClassifyError, ProjectKey, answer_count, answer_flag, mcp,
    without_rerank,
};
pub use rest::rest;

use prost::Message;

/// The browser transfer request `method path?query` with body `body`
/// ([`crate::browser::transfer`]): `POST /v1/import` (its query is the
/// import's settings; the archive is not read), `POST /v1/export` (its
/// JSON body is the export request), `POST /v1/blobs` (its query names the
/// workspace; the image is not read) and `POST /v1/blobs/link` (its JSON
/// body names the blob), classified as the `Import`, `Export`, `PutBlob`
/// and `GetBlob` RPCs they map to. A download (`GET` of a link) is no
/// operation.
///
/// # Errors
/// Another route, settings or a request that do not parse, or a field that
/// names no id.
pub fn browser(
    method: &str,
    path: &str,
    query: Option<&str>,
    body: &[u8],
) -> Result<Classified, ClassifyError> {
    use crate::browser::transfer::blobs::{link_request, upload_workspace};
    use crate::browser::transfer::{export_request, import_settings};
    use post_cortex_proto::pb;

    let malformed = |s: tonic::Status| ClassifyError::Malformed(s.message().to_string());
    match (method.to_ascii_uppercase().as_str(), path) {
        ("POST", "/v1/import") => {
            let first = pb::ImportChunk {
                data: Vec::new(),
                settings: Some(import_settings(query).map_err(malformed)?),
            };
            grpc("/pcx.v1.PostCortex/Import", &first.encode_to_vec())
        }
        ("POST", "/v1/export") => {
            let request = export_request(body).map_err(malformed)?;
            grpc("/pcx.v1.PostCortex/Export", &request.encode_to_vec())
        }
        ("POST", "/v1/blobs") => {
            let first = pb::PutBlobChunk {
                workspace_id: upload_workspace(query).map_err(malformed)?,
                data: Vec::new(),
                // An upload over HTTP is an image.
                media_type: String::new(),
            };
            grpc("/pcx.v1.Blobs/PutBlob", &first.encode_to_vec())
        }
        ("POST", "/v1/blobs/link") => {
            let request = link_request(body).map_err(malformed)?;
            grpc("/pcx.v1.Blobs/GetBlob", &request.encode_to_vec())
        }
        _ => Err(ClassifyError::UnknownRoute(format!("{method} {path}"))),
    }
}

/// The count in `field` of the data of an MCP tool result (`CallToolResult`
/// as JSON: the block after [`crate::mcp::RESULTS`]).
///
/// # Errors
/// The result carries no data, or the field is not a count.
pub fn mcp_answer_count(result: &Value, field: &str) -> Result<i64, ClassifyError> {
    answer_count(&mcp_data(result, field)?, field)
}

/// The flag in `field` of the data of an MCP tool result; `None` when the
/// data does not carry it (a `select` left it out).
///
/// # Errors
/// The result carries no data.
pub fn mcp_answer_flag(result: &Value, field: &str) -> Result<Option<bool>, ClassifyError> {
    Ok(answer_flag(&mcp_data(result, field)?, field))
}

/// The data of an MCP tool result, for reading `field` of it.
fn mcp_data(result: &Value, field: &str) -> Result<Value, ClassifyError> {
    let data = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .find_map(|text| text.strip_prefix(crate::mcp::RESULTS))
        .ok_or_else(|| ClassifyError::BadCount {
            field: field.to_string(),
        })?;
    serde_json::from_str(data).map_err(|e| ClassifyError::Malformed(e.to_string()))
}

#[cfg(test)]
mod tests;
