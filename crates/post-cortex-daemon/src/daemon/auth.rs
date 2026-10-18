// Copyright (c) 2025 Julius ML
// MIT License

//! Bearer-token authentication for the daemon's HTTP and gRPC endpoints.
//!
//! Enabled by `auth_token` in `daemon.toml` (or `PC_AUTH_TOKEN`). Without a
//! token the daemon stays open, which is only safe on a loopback bind; with
//! one, every request except `GET /health` must carry
//! `Authorization: Bearer <token>`.

use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// Paths reachable without a token: liveness probes only.
const PUBLIC_PATHS: &[&str] = &["/health"];

/// Constant-time comparison of an `Authorization` header value against the
/// expected token, so response timing does not leak how much of it matched.
#[must_use]
pub fn bearer_matches(header_value: Option<&str>, token: &str) -> bool {
    let Some(presented) = header_value.and_then(|v| v.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (presented.as_bytes(), token.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// axum middleware: rejects requests without the configured bearer token.
pub async fn require_bearer(
    State(token): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Response {
    if PUBLIC_PATHS.contains(&req.uri().path()) {
        return next.run(req).await;
    }
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if bearer_matches(presented, &token) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or invalid bearer token\n",
        )
            .into_response()
    }
}

/// tonic interceptor: same check on the `authorization` metadata.
pub fn grpc_interceptor(
    token: Arc<String>,
) -> impl Fn(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> + Clone {
    move |req: tonic::Request<()>| {
        let presented = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if bearer_matches(presented, &token) {
            Ok(req)
        } else {
            Err(tonic::Status::unauthenticated(
                "missing or invalid bearer token",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_matching() {
        assert!(bearer_matches(Some("Bearer s3cret"), "s3cret"));
        assert!(!bearer_matches(Some("Bearer s3creT"), "s3cret"));
        assert!(!bearer_matches(Some("Bearer s3cret2"), "s3cret"));
        assert!(!bearer_matches(Some("s3cret"), "s3cret"));
        assert!(!bearer_matches(Some("Basic s3cret"), "s3cret"));
        assert!(!bearer_matches(None, "s3cret"));
    }

    #[test]
    fn grpc_interceptor_checks_metadata() {
        let check = grpc_interceptor(Arc::new("tok".into()));
        let mut ok = tonic::Request::new(());
        ok.metadata_mut()
            .insert("authorization", "Bearer tok".parse().unwrap());
        assert!(check(ok).is_ok());
        let err = check(tonic::Request::new(())).unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }
}
