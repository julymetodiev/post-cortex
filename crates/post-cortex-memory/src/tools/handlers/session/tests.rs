// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use pcx_module_api::ToolOutput;
use std::sync::Arc;

/// resolve_project returns the same ids on a repeat and rejects a bad key.
#[tokio::test]
async fn resolve_project_over_mcp() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let memory = Arc::new(memory);
    let service = memory.clone();
    let resolve = |key: &str| {
        serde_json::json!({
            "action": "resolve_project", "key": key, "name": "repo", "workspace": "acme",
        })
    };
    // The second block is "Results:" and the response JSON.
    let data = |reply: ToolOutput| -> serde_json::Value { reply.data.unwrap() };

    let first = data(
        handle(
            &service,
            resolve("github.com/acme/repo"),
            Some("codex".into()),
        )
        .await
        .unwrap(),
    );
    assert_eq!(first["created"], true, "{first}");
    let again = data(
        handle(&service, resolve("github.com/acme/repo"), None)
            .await
            .unwrap(),
    );
    assert_eq!(again["created"], false);
    assert_eq!(again["session_id"], first["session_id"]);
    let ws = memory.workspaces.find_by_name("acme").unwrap();
    assert_eq!(ws.id.to_string(), first["workspace_id"].as_str().unwrap());

    assert!(handle(&service, resolve("a b"), None).await.is_err());
    let missing = serde_json::json!({"action": "resolve_project", "key": "k"});
    assert!(handle(&service, missing, None).await.is_err());
}
