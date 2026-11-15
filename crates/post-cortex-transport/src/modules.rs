// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The modules' gRPC services, served to the host's clients (W23,
//! ARCHITECTURE section 7, views): a call on a path no other service of
//! the instance serves passes the instance's check, then goes to the
//! host's channel of a client, which routes it by path to the module that
//! serves it. Its frames are forwarded, never decoded; a request that is
//! not gRPC (nor gRPC-Web) is not found.

use std::convert::Infallible;

use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use pcx_module_api::grpc::{GrpcChannel, refusal};
use tonic::Status;
use tonic::metadata::MetadataMap;
use tower::ServiceExt;
use tower::util::BoxCloneSyncService;

/// The service of every module's calls: `check` (the instance's
/// interceptor) on its metadata, then `channel`.
pub fn service<F>(
    channel: GrpcChannel,
    check: F,
) -> BoxCloneSyncService<Request<axum::body::Body>, Response, Infallible>
where
    F: Fn(tonic::Request<()>) -> Result<tonic::Request<()>, Status> + Clone + Send + Sync + 'static,
{
    BoxCloneSyncService::new(tower::service_fn(
        move |request: Request<axum::body::Body>| {
            let channel = channel.clone();
            let check = check.clone();
            async move {
                if !grpc(&request) {
                    let mut missing = Response::new(axum::body::Body::empty());
                    *missing.status_mut() = StatusCode::NOT_FOUND;
                    return Ok::<_, Infallible>(missing);
                }
                let (mut parts, body) = request.into_parts();
                let metadata = MetadataMap::from_headers(std::mem::take(&mut parts.headers));
                let checked = check(tonic::Request::from_parts(
                    metadata,
                    tonic::Extensions::default(),
                    (),
                ));
                let answer = match checked {
                    Err(status) => refusal(&status),
                    Ok(checked) => {
                        parts.headers = checked.into_parts().0.into_headers();
                        let request = Request::from_parts(parts, tonic::body::Body::new(body));
                        match channel.oneshot(request).await {
                            Ok(answer) => answer,
                            Err(e) => refusal(&Status::unavailable(e.to_string())),
                        }
                    }
                };
                Ok::<_, Infallible>(answer.map(axum::body::Body::new))
            }
        },
    ))
}

/// Whether `request` is a gRPC call (native or gRPC-Web).
fn grpc(request: &Request<axum::body::Body>) -> bool {
    request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("application/grpc"))
}
