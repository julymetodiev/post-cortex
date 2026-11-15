// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use std::sync::Arc;

async fn memory() -> (Arc<Memory>, tempfile::TempDir) {
    let (memory, dir) = crate::capabilities::test_support::rocksdb_memory().await;
    (Arc::new(memory), dir)
}

/// Empty ids are missing ids: no scope is an invalid request, never a
/// panic.
#[tokio::test]
async fn empty_ids_without_a_scope_are_invalid_params() {
    let (memory, _dir) = memory().await;
    for body in [
        serde_json::json!({"query": "x", "workspace_id": ""}),
        serde_json::json!({"query": "x", "workspace_id": " ", "session_id": ""}),
    ] {
        let err = handle(&memory, body.clone(), None).await.unwrap_err();
        assert_eq!(err.kind, crate::tools::Kind::InvalidArgument, "{body}");
    }
}

/// An empty workspace id next to a session id scopes to the session.
#[tokio::test]
async fn an_empty_workspace_id_falls_back_to_the_session() {
    let (memory, _dir) = memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let body = serde_json::json!({
        "query": "x", "workspace_id": "", "session_id": sid.to_string(),
    });
    let reply = handle(&memory, body, None).await.unwrap();
    assert!(!reply.failed);
}

/// An unknown session or workspace is the same failure, not found (review
/// W4 item 9: an unknown session was an internal error, a workspace invalid
/// params).
#[tokio::test]
async fn unknown_scopes_are_not_found() {
    let (memory, _dir) = memory().await;
    let ghost = uuid::Uuid::new_v4().to_string();
    for body in [
        serde_json::json!({"query": "x", "session_id": ghost}),
        serde_json::json!({"query": "x", "workspace_id": ghost}),
    ] {
        let err = handle(&memory, body.clone(), None).await.unwrap_err();
        assert_eq!(err.kind, crate::tools::Kind::NotFound, "{body}");
        assert!(err.message.contains("not found"), "{}", err.message);
    }
}
