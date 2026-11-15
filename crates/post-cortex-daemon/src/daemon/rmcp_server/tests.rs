// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::daemon::DaemonConfig;
use axum::body::{Body, to_bytes};
use axum::http::StatusCode;
use axum::http::{Request, header};
use tower::ServiceExt;
use uuid::Uuid;

pub(super) const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"host-test","version":"0"}}}"#;

pub(super) async fn memory_system(dir: &tempfile::TempDir) -> Arc<Memory> {
    Arc::new(
        crate::memory_tests::rocksdb_memory(dir, post_cortex_memory::SystemConfig::default())
            .await
            .unwrap(),
    )
}

pub(super) fn policy(host: &str, allowed: &[&str]) -> HostPolicy {
    HostPolicy::from_config(&DaemonConfig {
        host: host.to_string(),
        allowed_hosts: allowed.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    })
    .unwrap()
}

pub(super) fn initialize(host: &str) -> Request<Body> {
    Request::post("/mcp")
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .body(Body::from(INITIALIZE))
        .unwrap()
}

pub(super) fn get(path: &str, host: &str) -> Request<Body> {
    Request::get(path)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap()
}

pub(super) async fn status_and_body(router: &Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    // Initialize answers on an SSE stream that stays open; the first
    // frame holds the result.
    let body = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        to_bytes(resp.into_body(), 64 * 1024),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .map(|b| String::from_utf8_lossy(&b).into_owned())
    .unwrap_or_default();
    (status, body)
}

#[tokio::test]
async fn host_allowlist_guards_mcp_and_api() {
    let dir = tempfile::tempdir().unwrap();
    let memory = memory_system(&dir).await;
    let ct = CancellationToken::new();
    let router = crate::daemon::test_client::router(
        memory.clone(),
        &policy("0.0.0.0", &["pc.tailnet.example", "100.64.0.7:3737"]),
        None,
        &ct,
    )
    .await;

    // DNS rebinding: foreign Host gets 403 on /mcp and /api/*.
    for host in ["evil.example", "evil.example:3737", "100.64.0.7:80"] {
        let (status, body) = status_and_body(&router, initialize(host)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "/mcp with Host {host}");
        assert!(body.contains("Host header is not allowed"), "{body}");
        let (status, _) = status_and_body(&router, get("/api/sessions", host)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "/api/sessions with Host {host}"
        );
    }

    // Loopback and configured hosts are served.
    for host in [
        "localhost:3737",
        "127.0.0.1:3737",
        "[::1]:3737",
        "pc.tailnet.example:3737",
        "100.64.0.7:3737",
    ] {
        let (status, body) = status_and_body(&router, initialize(host)).await;
        assert_eq!(status, StatusCode::OK, "/mcp with Host {host}");
        assert!(
            body.contains("\"protocolVersion\":\"2024-11-05\""),
            "{body}"
        );
        let (status, _) = status_and_body(&router, get("/api/sessions", host)).await;
        assert_eq!(status, StatusCode::OK, "/api/sessions with Host {host}");
    }

    // Liveness probe stays open.
    let (status, _) = status_and_body(&router, get("/health", "evil.example")).await;
    assert_eq!(status, StatusCode::OK);
    ct.cancel();
}

pub(super) fn post_json(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(path)
        .header(header::HOST, "localhost:3737")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// `POST /api/messages` stores turns through the canonical service:
/// unknown sessions are 404, secrets are redacted, retries are skipped.
#[tokio::test]
async fn api_messages_append_redacted_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ct = CancellationToken::new();
    let router =
        crate::daemon::test_client::router(memory.clone(), &policy("127.0.0.1", &[]), None, &ct)
            .await;
    let body = |session: Uuid| {
        serde_json::json!({
            "session_id": session,
            "messages": [
                {"turn_id": "u1", "role": "user", "text": "deploy with key AKIAABCDEFGHIJKLMNOP"},
                {"turn_id": "a1", "role": "assistant", "text": "done", "author": "claude",
                 "tools": ["Bash"], "files": ["deploy.sh"]},
                {"turn_id": "a2", "role": "assistant", "text": "   "}
            ]
        })
    };

    let (status, _) =
        status_and_body(&router, post_json("/api/messages", body(Uuid::new_v4()))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let sid = memory.sessions.create(None, None).await.unwrap();
    let (status, text) = status_and_body(&router, post_json("/api/messages", body(sid))).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let response: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(response["appended"], serde_json::json!([0, 1]));
    assert_eq!(response["skipped"], 1, "the empty turn");

    let (status, text) = status_and_body(&router, post_json("/api/messages", body(sid))).await;
    assert_eq!(status, StatusCode::OK);
    let response: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(response["appended"], serde_json::json!([]));
    assert_eq!(response["skipped"], 3);

    let stored = memory
        .messages
        .page(
            sid,
            post_cortex_core::core::message::MessageCursor::From(0),
            10,
            post_cortex_core::core::message::MessageOrder::Asc,
        )
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert!(!stored[0].text.contains("AKIA"), "{}", stored[0].text);
    assert!(stored[0].text.contains("[REDACTED:"), "{}", stored[0].text);
    assert_eq!(stored[1].author.as_deref(), Some("claude"));
    assert_eq!(stored[1].tools, ["Bash"]);

    let (status, _) = status_and_body(
        &router,
        post_json(
            "/api/messages",
            serde_json::json!({"session_id": sid, "messages": [{"turn_id": "x", "text": "no role"}]}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    ct.cancel();
}

pub(super) fn put_json(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::put(path)
        .header(header::HOST, "localhost:3737")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Cards over REST: versions, 409 with the latest on a stale write, 404
/// for an unknown workspace, and GET returning both cards.
#[tokio::test]
async fn api_cards_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ct = CancellationToken::new();
    let router =
        crate::daemon::test_client::router(memory.clone(), &policy("127.0.0.1", &[]), None, &ct)
            .await;
    let ws = memory
        .workspaces
        .create(post_cortex_memory::capabilities::CreateWorkspace {
            name: "cards".to_string(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let put = |path: String, content: &str, expected_version: u64| {
        put_json(
            &path,
            serde_json::json!({"content": content, "expected_version": expected_version}),
        )
    };
    let json = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();

    let (status, _) = status_and_body(
        &router,
        put(format!("/api/cards/workspace/{}", Uuid::new_v4()), "x", 0),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let path = format!("/api/cards/workspace/{ws}");
    let (status, text) = status_and_body(&router, put(path.clone(), "## Stack\nrust", 0)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(json(&text)["card"]["version"], 1);

    let (status, text) = status_and_body(&router, put(path.clone(), "stale", 0)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json(&text)["card"]["text"], "## Stack\nrust");

    let mut by_header = put("/api/cards/user".to_string(), "be brief", 0);
    by_header
        .headers_mut()
        .insert("x-pcx-author", "claude-code".parse().unwrap());
    let (status, text) = status_and_body(&router, by_header).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&text)["card"]["author"], "claude-code");

    let (status, text) = status_and_body(
        &router,
        get(&format!("/api/cards?workspace_id={ws}"), "localhost:3737"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let cards = json(&text);
    assert_eq!(cards["user"]["text"], "be brief");
    assert_eq!(cards["workspace"]["version"], 1);
    assert_eq!(cards["workspace"]["scope"]["workspace"], ws.to_string());

    let (_, text) = status_and_body(&router, get("/api/cards", "localhost:3737")).await;
    assert!(json(&text)["workspace"].is_null());
    ct.cancel();
}

/// `POST /api/projects/resolve` answers with the contract's shape and
/// the same ids on a repeat; bad keys are 400.
#[tokio::test]
async fn api_resolve_project_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ct = CancellationToken::new();
    let router =
        crate::daemon::test_client::router(memory.clone(), &policy("127.0.0.1", &[]), None, &ct)
            .await;
    let body = |key: &str| {
        serde_json::json!({
            "key": key, "name": "post-cortex", "client": "claude-code"
        })
    };
    let resolve = |key: &str| post_json("/api/projects/resolve", body(key));

    let (status, text) = status_and_body(&router, resolve("github.com/a/b")).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let first: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(first["created"], true);
    let mut keys: Vec<_> = first.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["created", "session_id", "workspace_id"]);

    let (_, text) = status_and_body(&router, resolve("github.com/a/b")).await;
    let again: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(again["created"], false);
    assert_eq!(again["workspace_id"], first["workspace_id"]);
    assert_eq!(again["session_id"], first["session_id"]);

    let (status, _) = status_and_body(&router, resolve("")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    ct.cancel();
}

#[tokio::test]
async fn star_disables_host_validation() {
    let dir = tempfile::tempdir().unwrap();
    let ct = CancellationToken::new();
    let memory = memory_system(&dir).await;
    let router =
        crate::daemon::test_client::router(memory.clone(), &policy("0.0.0.0", &["*"]), None, &ct)
            .await;
    let (status, _) = status_and_body(&router, initialize("evil.example")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = status_and_body(&router, get("/api/sessions", "evil.example")).await;
    assert_eq!(status, StatusCode::OK);
    ct.cancel();
}
