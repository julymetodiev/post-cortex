// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Blobs for browsers (W24): an image uploaded as a raw body, and a
//! single-use link to download one (an `<img>` sends no `Authorization`).
//! Each maps onto the Blobs capability as the `PutBlob` and `GetBlob` RPCs
//! do.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use post_cortex_core::core::blob::MAX_BLOB_BYTES;
use post_cortex_proto::pb;
use serde::Deserialize;
use serde_json::json;
use tonic::Status;

use crate::browser::transfer::BrowserTransfer;
use crate::grpc::http_failure;

/// Largest JSON body of `POST /v1/blobs/link`.
const MAX_LINK_REQUEST_BYTES: usize = 4 * 1024;

/// The workspace of a `POST /v1/blobs` query (`workspace_id=`).
///
/// # Errors
/// No workspace, or another parameter.
pub fn upload_workspace(query: Option<&str>) -> Result<String, Status> {
    let mut workspace = None;
    for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "workspace_id" => workspace = Some(value.into_owned()),
            other => {
                return Err(Status::invalid_argument(format!(
                    "unknown upload parameter '{other}'"
                )));
            }
        }
    }
    workspace.ok_or_else(|| Status::invalid_argument("the upload names no workspace_id"))
}

/// The JSON body of `POST /v1/blobs/link`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkBody {
    workspace_id: String,
    sha256: String,
}

/// The blob a `POST /v1/blobs/link` body names.
///
/// # Errors
/// The body is not `{"workspace_id", "sha256"}`.
pub fn link_request(body: &[u8]) -> Result<pb::BlobRequest, Status> {
    let body: LinkBody = serde_json::from_slice(body)
        .map_err(|e| Status::invalid_argument(format!("not a blob link request: {e}")))?;
    Ok(pb::BlobRequest {
        workspace_id: body.workspace_id,
        sha256: body.sha256,
    })
}

/// `POST /v1/blobs?workspace_id=`: the raw body kept as an image.
pub async fn upload(
    State(transfer): State<Arc<BrowserTransfer>>,
    RawQuery(query): RawQuery,
    body: Body,
) -> Response {
    let workspace = match upload_workspace(query.as_deref()) {
        Ok(workspace) => workspace,
        Err(s) => return http_failure(&s),
    };
    // One byte over the limit is enough for the capability to refuse.
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BLOB_BYTES + 1).await else {
        return http_failure(&Status::invalid_argument(format!(
            "the image is over {MAX_BLOB_BYTES} bytes"
        )));
    };
    // The app puts images only.
    match crate::blobs::put(&transfer.memory, &workspace, bytes.to_vec(), "").await {
        Ok(info) => axum::Json(json!({
            "sha256": info.sha256,
            "size": info.size,
            "media_type": info.media_type,
            "width": info.width,
            "height": info.height,
        }))
        .into_response(),
        Err(s) => http_failure(&s),
    }
}

/// `POST /v1/blobs/link`: a single-use link to a blob.
pub async fn link(State(transfer): State<Arc<BrowserTransfer>>, body: Body) -> Response {
    let Ok(body) = axum::body::to_bytes(body, MAX_LINK_REQUEST_BYTES).await else {
        return http_failure(&Status::invalid_argument("blob link request too large"));
    };
    let named = match link_request(&body) {
        Ok(named) => named,
        Err(s) => return http_failure(&s),
    };
    match transfer.blobs.issue(named) {
        Ok(ticket) => axum::Json(json!({
            "url": transfer.link("blobs", &ticket),
            "expires_in": transfer.blobs.ttl().as_secs(),
        }))
        .into_response(),
        Err(s) => http_failure(&s),
    }
}

/// `GET /v1/blobs/{ticket}`: the image of a link, once.
pub async fn download(
    State(transfer): State<Arc<BrowserTransfer>>,
    Path(ticket): Path<String>,
) -> Response {
    let Some(named) = transfer.blobs.take(&ticket) else {
        return (
            StatusCode::NOT_FOUND,
            "this image link expired or was already used; ask for a new one\n",
        )
            .into_response();
    };
    let blob = match crate::blobs::get(&transfer.memory, &named.workspace_id, &named.sha256).await {
        Ok(blob) => blob,
        Err(s) => return http_failure(&s),
    };
    let mut response = Response::new(Body::from(blob.bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(blob.info.image.media_type.as_str()),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
