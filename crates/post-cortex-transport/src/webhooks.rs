// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `POST /v1/webhooks/{source}` (ARCHITECTURE section 8, operations, W27):
//! the request whole (its source, its bearer token, its body as bytes) to
//! the webhooks module's `Receive` through the host's channel of a client,
//! and its answer as an HTTP status. Nothing is decoded here: the module
//! checks the token and reads the body. Not behind the instance's own
//! token: a source authenticates with the webhooks' token.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use pcx_module_api::grpc::GrpcChannel;
use post_cortex_proto::webhooks::ReceiveRequest;
use post_cortex_proto::webhooks::webhooks_client::WebhooksClient;

use crate::grpc::http_failure;

/// The webhooks' route over `channel`, the host's channel of a client.
pub fn routes(channel: GrpcChannel) -> Router {
    Router::new()
        .route("/v1/webhooks/{source}", post(receive))
        .with_state(channel)
}

/// The bearer token of `headers`; empty: none (a header that is not text
/// or not a bearer carries none, which the module refuses).
fn bearer(headers: &HeaderMap) -> String {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return String::new();
    };
    match value.to_str().map(|v| v.strip_prefix("Bearer ")) {
        Ok(Some(token)) => token.trim().to_string(),
        Ok(None) | Err(_) => String::new(),
    }
}

async fn receive(
    State(channel): State<GrpcChannel>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = ReceiveRequest {
        source,
        token: bearer(&headers),
        body: body.to_vec(),
    };
    match WebhooksClient::new(channel).receive(request).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(status) => http_failure(&status),
    }
}
