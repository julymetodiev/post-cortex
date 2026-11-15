// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The built-in presets read and checked, and the client against a server
//! on loopback: what it sends, what it reads, the limit of a body.

use std::net::SocketAddr;

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use post_cortex_proto::http::{Auth as AuthKind, Connection as Wire, HttpConfig, HttpRequest};

use super::*;
use crate::domain::request::plan;
use crate::domain::settings;
use crate::ports::Client;

#[test]
fn the_built_in_presets_read_and_check() {
    let presets = built_in().unwrap();
    let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
    for name in [
        "rest-get",
        "github-issue-create",
        "slack-message-post",
        "stripe-invoices-list",
    ] {
        assert!(names.contains(&name), "{name}");
    }
    let stripe: Vec<_> = presets.iter().filter(|p| p.service == "stripe").collect();
    assert!(
        stripe.iter().all(|p| p.method == "GET"),
        "stripe's presets only read"
    );
}

/// A server on loopback: `/echo` answers the authorization and the user
/// agent it was sent,
/// `/big` a body of 3 MiB.
async fn server() -> SocketAddr {
    let app = Router::new()
        .route(
            "/api/echo",
            post(|headers: HeaderMap, body: String| async move {
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                };
                let answer = serde_json::json!({
                    "auth": header("authorization"),
                    "agent": header("user-agent"),
                    "body": body,
                });
                ([("content-type", "application/json")], answer.to_string())
            }),
        )
        .route("/api/big", get(|| async { "x".repeat(3 << 20) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    at
}

fn through(at: SocketAddr) -> crate::domain::Settings {
    settings(
        HttpConfig {
            workspace_id: "6f1c2a3e-0000-4000-8000-000000000001".into(),
            connections: vec![Wire {
                name: "local".into(),
                base_url: format!("http://127.0.0.1:{}/api", at.port()),
                auth: AuthKind::Bearer.into(),
                secret: "tok".into(),
                ..Wire::default()
            }],
            private_networks: true,
            ..HttpConfig::default()
        },
        false,
    )
    .unwrap()
}

#[tokio::test]
async fn the_client_sends_to_the_checked_address_and_reads_the_answer() {
    let at = server().await;
    let s = through(at);
    let request = HttpRequest {
        connection: "local".into(),
        method: "POST".into(),
        path: "/echo".into(),
        body: Some(post_cortex_proto::http::http_request::Body::Text(
            "hi".into(),
        )),
        ..HttpRequest::default()
    };
    let planned = plan(&request, &s, false, None).unwrap();
    let received = Reqwest.send(&planned, &[at], 1 << 20).await.unwrap();
    assert_eq!(received.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({ "auth": "Bearer tok", "agent": format!("pcx-http/{}", env!("CARGO_PKG_VERSION")), "body": "hi" })
    );
}

#[tokio::test]
async fn a_body_above_the_limit_is_refused() {
    let at = server().await;
    let s = through(at);
    let request = HttpRequest {
        connection: "local".into(),
        method: "GET".into(),
        path: "/big".into(),
        ..HttpRequest::default()
    };
    let planned = plan(&request, &s, true, None).unwrap();
    let refused = Reqwest.send(&planned, &[at], 2 << 20).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::ResourceExhausted);
}
