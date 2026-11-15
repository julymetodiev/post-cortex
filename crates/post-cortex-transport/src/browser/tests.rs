// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The origin allowlist is parsed strictly and answers preflights only for
//! the origins it allows.

use super::*;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use tower::ServiceExt;

fn entries(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn origins_are_parsed_strictly() {
    let origins = Origins::parse(&entries(&["https://app.example.com/", "localhost"])).unwrap();
    let allows = |o: &str| origins.allows(&HeaderValue::from_str(o).unwrap());
    assert!(allows("https://app.example.com"));
    assert!(allows("http://localhost:5173"));
    assert!(allows("http://127.0.0.1"));
    assert!(!allows("https://evil.example.com"));
    assert!(!allows("http://localhost.evil.com"));
    assert!(!allows("http://localhost:"));
    for bad in [
        "app.example.com",
        "https://",
        "ftp://x",
        "https://a/b",
        "https://a b",
    ] {
        assert_eq!(
            Origins::parse(&entries(&[bad])),
            Err(InvalidOrigin(bad.to_string())),
            "{bad}"
        );
    }
    assert!(Origins::parse(&[]).unwrap().is_empty());
}

#[tokio::test]
async fn preflights_are_answered_for_allowed_origins_only() {
    let origins = Origins::parse(&entries(&["https://app.example.com"])).unwrap();
    let app = Router::new()
        .route("/v1/import", post(|| async { "ok" }))
        .layer(cors(origins));
    let preflight = |origin: &str| {
        Request::builder()
            .method("OPTIONS")
            .uri("/v1/import")
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "authorization")
            .body(Body::empty())
            .unwrap()
    };
    let allowed = app
        .clone()
        .oneshot(preflight("https://app.example.com"))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        allowed.headers()["access-control-allow-origin"],
        "https://app.example.com"
    );
    let refused = app
        .oneshot(preflight("https://evil.example.com"))
        .await
        .unwrap();
    assert!(
        !refused
            .headers()
            .contains_key("access-control-allow-origin")
    );
}
