// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `POST /api/events`: hook events reach the memory module through the
//! host, and the answer is a Claude Code hook answer.

use super::*;
use crate::daemon::rmcp_server::tests::{policy, post_json, status_and_body};
use axum::http::StatusCode;
use post_cortex_memory::capabilities::CreateWorkspace;

async fn app() -> (Router, Arc<Memory>, tempfile::TempDir, CancellationToken) {
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
    (router, memory, dir, ct)
}

#[tokio::test]
async fn session_start_answers_with_the_cards() {
    let (router, memory, _dir, ct) = app().await;
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "p".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let body = serde_json::json!({
        "scope": {"workspace_id": ws.id.to_string()},
        "event": {
            "session_id": "client-1", "cwd": "/repo", "permission_mode": "default",
            "hook_event_name": "SessionStart", "source": "startup"
        }
    });
    let (status, text) = status_and_body(&router, post_json("/api/events", body)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let answer: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        answer["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    let context = answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.starts_with("## post-cortex cards"), "{context}");
    ct.cancel();
}

/// A module failure fails open: the session goes on with an empty answer.
#[tokio::test]
async fn a_failing_event_fails_open() {
    let (router, _memory, _dir, ct) = app().await;
    let body = serde_json::json!({
        "scope": {},
        "event": {"hook_event_name": "SessionStart"}
    });
    let (status, text) = status_and_body(&router, post_json("/api/events", body)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(text, "{}");
    ct.cancel();
}

#[tokio::test]
async fn an_envelope_without_its_event_or_a_known_hook_is_refused() {
    let (router, _memory, _dir, ct) = app().await;
    for body in [
        serde_json::json!({"scope": {}}),
        serde_json::json!({"scope": {}, "event": {"hook_event_name": "Notification"}}),
        serde_json::json!({"event": {"hook_event_name": "SessionStart"}}),
    ] {
        let (status, text) = status_and_body(&router, post_json("/api/events", body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    }
    ct.cancel();
}
