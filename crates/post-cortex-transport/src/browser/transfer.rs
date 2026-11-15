// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Export and import for browsers: plain HTTP over the same mapping as the
//! `Export` and `Import` RPCs ([`crate::transfer`]). gRPC-Web cannot stream
//! a request from a browser, and a download a page assembles in memory
//! breaks on large archives. So:
//!
//! - **Import**: `POST /v1/import?commit=&skip_existing=&overwrite=&cards_newer=&session_id=&workspace_id=`
//!   with the archive as the raw body, spooled as it arrives (never held
//!   whole; the instance's `max_import_bytes`). Without `commit=true` it is
//!   a preview. The answer is the `ImportResponse` as JSON.
//! - **Export**: `POST /v1/export` with the `ExportRequest` as JSON answers
//!   a single-use download link (`{"url": "/v1/export/<ticket>",
//!   "filename", "expires_in"}`); `GET` of the link streams the file as an
//!   attachment, so the browser saves it itself. A navigation carries no
//!   `Authorization`, so the link is its credential ([`Tickets`]:
//!   [`downloads`] is served without the token; [`routes`] with it).
//! - **Blobs** (W24, [`blobs`]): `POST /v1/blobs?workspace_id=` with an
//!   image as the raw body answers what it is (`BlobInfo` as JSON);
//!   `POST /v1/blobs/link` with `{"workspace_id", "sha256"}` answers a
//!   single-use link (`{"url": "/v1/blobs/<ticket>", "expires_in"}`) whose
//!   `GET` is the image.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::StreamExt;
use post_cortex_memory::Memory;
use post_cortex_proto::pb;
use serde::Deserialize;
use serde_json::{Value, json};
use tonic::Status;

use crate::browser::tickets::Tickets;
use crate::grpc::http_failure;

/// Largest JSON body of `POST /v1/export`.
const MAX_EXPORT_REQUEST_BYTES: usize = 64 * 1024;

/// How long a download link is valid by default.
pub const DEFAULT_LINK_TTL: Duration = Duration::from_secs(300);

/// What an export link downloads.
struct Ticket {
    request: pb::ExportRequest,
    filename: String,
}

/// Where download links point by default: `/v1/export/<ticket>`.
pub const DEFAULT_LINK_BASE: &str = "/v1";

/// Browser transfer over one memory instance, with its pending links.
pub struct BrowserTransfer {
    memory: Arc<Memory>,
    exports: Tickets<Ticket>,
    blobs: Tickets<pb::BlobRequest>,
    link_base: String,
}

impl BrowserTransfer {
    /// Transfer over `memory`; links are valid for `ttl` and point under
    /// `link_base` (`<link_base>/export/<ticket>`: [`DEFAULT_LINK_BASE`] for
    /// an instance a browser reaches directly, the path a gateway routes to
    /// the instance otherwise).
    pub fn new(memory: Arc<Memory>, ttl: Duration, link_base: String) -> Arc<Self> {
        Arc::new(Self {
            memory,
            exports: Tickets::new(ttl),
            blobs: Tickets::new(ttl),
            link_base,
        })
    }
}

/// `POST /v1/import`, `POST /v1/export`, `POST /v1/blobs` and
/// `POST /v1/blobs/link`, to serve behind the token.
pub fn routes(transfer: Arc<BrowserTransfer>) -> Router {
    Router::new()
        .route("/v1/import", post(import))
        .route("/v1/export", post(export_link))
        .route("/v1/blobs", post(blobs::upload))
        .route("/v1/blobs/link", post(blobs::link))
        .with_state(transfer)
}

/// `GET /v1/export/{ticket}` and `GET /v1/blobs/{ticket}`: the link is the
/// credential.
pub fn downloads(transfer: Arc<BrowserTransfer>) -> Router {
    Router::new()
        .route("/v1/export/{ticket}", get(download))
        .route("/v1/blobs/{ticket}", get(blobs::download))
        .with_state(transfer)
}

/// `true`/`1` or `false`/`0`.
fn flag(key: &str, value: &str) -> Result<bool, Status> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(Status::invalid_argument(format!(
            "{key} must be true or false, got '{other}'"
        ))),
    }
}

/// The import settings of a `POST /v1/import` query.
///
/// # Errors
/// An unknown parameter, or a flag that is not true or false.
pub fn import_settings(query: Option<&str>) -> Result<pb::ImportSettings, Status> {
    let mut settings = pb::ImportSettings::default();
    for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "commit" => settings.commit = flag(&key, &value)?,
            "skip_existing" => settings.skip_existing = flag(&key, &value)?,
            "overwrite" => settings.overwrite = flag(&key, &value)?,
            "cards_newer" => settings.cards_newer = flag(&key, &value)?,
            "session_id" => settings.session_ids.push(value.into_owned()),
            "workspace_id" => settings.workspace_ids.push(value.into_owned()),
            other => {
                return Err(Status::invalid_argument(format!(
                    "unknown import parameter '{other}'"
                )));
            }
        }
    }
    Ok(settings)
}

/// The JSON body of `POST /v1/export`: the fields of `ExportRequest`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportBody {
    #[serde(default)]
    scope: String,
    #[serde(default)]
    session_ids: Vec<String>,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    include_checkpoints: bool,
    #[serde(default)]
    compression: String,
    #[serde(default)]
    no_messages: bool,
}

/// The export request of a `POST /v1/export` body (empty: everything),
/// checked as the `Export` RPC checks it.
///
/// # Errors
/// The body is not the JSON of an export request, or the request is
/// invalid.
pub fn export_request(body: &[u8]) -> Result<pb::ExportRequest, Status> {
    let body: ExportBody = if body.is_empty() {
        ExportBody::default()
    } else {
        serde_json::from_slice(body)
            .map_err(|e| Status::invalid_argument(format!("not an export request: {e}")))?
    };
    let request = pb::ExportRequest {
        scope: body.scope,
        session_ids: body.session_ids,
        workspace_id: body.workspace_id,
        include_checkpoints: body.include_checkpoints,
        compression: body.compression,
        no_messages: body.no_messages,
    };
    crate::transfer::export_parts(&request)?;
    Ok(request)
}

/// An `ImportResponse` as JSON (the proto's field names).
pub fn import_json(r: &pb::ImportResponse) -> Value {
    let head = |h: &Option<pb::ImportCardHead>| {
        h.as_ref().map(|h| {
            json!({
                "version": h.version,
                "updated_at_unix": h.updated_at_unix,
                "first_line": h.first_line,
            })
        })
    };
    json!({
        "committed": r.committed,
        "format_version": r.format_version,
        "sessions": r.sessions,
        "workspaces": r.workspaces,
        "updates": r.updates,
        "checkpoints": r.checkpoints,
        "embeddings": r.embeddings,
        "messages": r.messages,
        "messages_new": r.messages_new,
        "cards": r.cards,
        "warnings": r.warnings,
        "conflicts": r.conflicts.iter().map(|c| json!({
            "kind": c.kind,
            "id": c.id,
            "name": c.name,
            "resolution": c.resolution,
            "target": head(&c.target),
            "archive": head(&c.archive),
        })).collect::<Vec<_>>(),
        "sessions_imported": r.sessions_imported,
        "sessions_skipped": r.sessions_skipped,
        "workspaces_imported": r.workspaces_imported,
        "workspaces_skipped": r.workspaces_skipped,
        "updates_imported": r.updates_imported,
        "checkpoints_imported": r.checkpoints_imported,
        "embeddings_imported": r.embeddings_imported,
        "messages_imported": r.messages_imported,
        "messages_skipped": r.messages_skipped,
        "cards_imported": r.cards_imported,
        "cards_skipped": r.cards_skipped,
        "errors": r.errors,
    })
}

async fn import(
    State(transfer): State<Arc<BrowserTransfer>>,
    RawQuery(query): RawQuery,
    body: Body,
) -> Response {
    let settings = match import_settings(query.as_deref()) {
        Ok(settings) => settings,
        Err(s) => return http_failure(&s),
    };
    let first = pb::ImportChunk {
        data: Vec::new(),
        settings: Some(settings),
    };
    let rest = body.into_data_stream().map(|data| {
        data.map(|data| pb::ImportChunk {
            data: data.to_vec(),
            settings: None,
        })
        .map_err(|e| Status::invalid_argument(format!("upload interrupted: {e}")))
    });
    let chunks = Box::pin(futures::stream::once(async { Ok(first) }).chain(rest));
    match crate::transfer::import(&transfer.memory, chunks).await {
        Ok(response) => axum::Json(import_json(&response)).into_response(),
        Err(s) => http_failure(&s),
    }
}

/// File name of an export: `post-cortex-<scope>-<UTC time>.json[.gz|.zst]`.
fn filename(request: &pb::ExportRequest) -> String {
    let ext = match request.compression.trim().to_ascii_lowercase().as_str() {
        "gzip" | "gz" => "json.gz",
        "zstd" | "zst" => "json.zst",
        _ => "json",
    };
    let scope = match request.scope.trim() {
        "" => "all",
        scope => scope,
    };
    let at = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    format!("post-cortex-{}-{at}.{ext}", scope.to_ascii_lowercase())
}

async fn export_link(State(transfer): State<Arc<BrowserTransfer>>, body: Body) -> Response {
    let Ok(body) = axum::body::to_bytes(body, MAX_EXPORT_REQUEST_BYTES).await else {
        return http_failure(&Status::invalid_argument("export request too large"));
    };
    let request = match export_request(&body) {
        Ok(request) => request,
        Err(s) => return http_failure(&s),
    };
    let name = filename(&request);
    let ticket = match transfer.exports.issue(Ticket {
        request,
        filename: name.clone(),
    }) {
        Ok(ticket) => ticket,
        Err(s) => return http_failure(&s),
    };
    axum::Json(json!({
        "url": transfer.link("export", &ticket),
        "filename": name,
        "expires_in": transfer.exports.ttl().as_secs(),
    }))
    .into_response()
}

fn content_type(compression: &str) -> &'static str {
    match compression.trim().to_ascii_lowercase().as_str() {
        "gzip" | "gz" => "application/gzip",
        "zstd" | "zst" => "application/zstd",
        _ => "application/json",
    }
}

async fn download(
    State(transfer): State<Arc<BrowserTransfer>>,
    Path(ticket): Path<String>,
) -> Response {
    let Some(ticket) = transfer.exports.take(&ticket) else {
        return (
            StatusCode::NOT_FOUND,
            "this export link expired or was already used; start the export again\n",
        )
            .into_response();
    };
    let mut chunks = match crate::transfer::export(&transfer.memory, &ticket.request).await {
        Ok(chunks) => chunks,
        Err(s) => return http_failure(&s),
    };
    // The first chunk before the headers: a refused export is an HTTP
    // error, not a broken download.
    let first = match chunks.next().await {
        Some(Ok(chunk)) => chunk,
        Some(Err(s)) => return http_failure(&s),
        None => pb::ExportChunk::default(),
    };
    // A failure after the first chunk aborts the body: the browser marks
    // the download failed instead of saving a cut file.
    let rest = chunks.map(|chunk| {
        chunk
            .map(|c| Bytes::from(c.data))
            .map_err(|s| std::io::Error::other(s.message().to_string()))
    });
    let body =
        futures::stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from(first.data)) })
            .chain(rest);
    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&ticket.request.compression)),
    );
    match HeaderValue::from_str(&format!("attachment; filename=\"{}\"", ticket.filename)) {
        Ok(value) => {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
        Err(e) => tracing::warn!("export file name not sent: {e}"),
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

impl BrowserTransfer {
    /// The link of `ticket` under `kind` (`export`, `blobs`).
    fn link(&self, kind: &str, ticket: &str) -> String {
        format!("{}/{kind}/{ticket}", self.link_base.trim_end_matches('/'))
    }
}

pub mod blobs;

#[cfg(test)]
mod tests;
