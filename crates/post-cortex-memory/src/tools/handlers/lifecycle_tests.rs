// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The MCP actions for workspace lifecycle (manage_workspace update /
//! archive / unarchive / list include_archived), radar dismissals
//! (dismiss_radar_item / restore_radar_item / list_radar_dismissals and the
//! radar view's include_dismissed) and message deletion (session
//! delete_message / delete_messages): the same operations as the gRPC RPCs.

use std::sync::Arc;

use crate::Memory;
use pcx_module_api::ToolOutput;
use post_cortex_core::core::message::{MessageRole, NewMessage};

async fn service() -> (Arc<Memory>, tempfile::TempDir) {
    let (memory, dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let memory = Arc::new(memory);
    (memory, dir)
}

/// The data of a reply.
fn data(reply: ToolOutput) -> serde_json::Value {
    reply.data.expect("a reply with data")
}

async fn workspace(svc: &Arc<Memory>, body: serde_json::Value) -> serde_json::Value {
    data(
        crate::tools::handlers::workspace::handle(svc, body, Some("claude-code".into()), None)
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn workspace_actions_update_archive_and_list() {
    let (svc, _dir) = service().await;
    let created = workspace(
        &svc,
        serde_json::json!({"action": "create", "name": "ws", "description": "d"}),
    )
    .await;
    let ws = created["workspace_id"].as_str().unwrap().to_string();

    let updated = workspace(
        &svc,
        serde_json::json!({"action": "update", "workspace_id": ws, "name": "renamed"}),
    )
    .await;
    assert_eq!(updated["name"], "renamed");
    assert!(
        crate::tools::handlers::workspace::handle(
            &svc,
            serde_json::json!({"action": "update", "workspace_id": ws}),
            None,
            None,
        )
        .await
        .is_err(),
        "nothing to update"
    );

    let archived = workspace(
        &svc,
        serde_json::json!({"action": "archive", "workspace_id": ws}),
    )
    .await;
    assert_eq!(archived["changed"], true);
    assert!(archived["archived_at"].is_string());
    let listed = workspace(&svc, serde_json::json!({"action": "list"})).await;
    assert!(listed["workspaces"].as_array().unwrap().is_empty());
    let all = workspace(
        &svc,
        serde_json::json!({"action": "list", "include_archived": true}),
    )
    .await;
    assert_eq!(all["workspaces"][0]["name"], "renamed");
    let back = workspace(
        &svc,
        serde_json::json!({"action": "unarchive", "workspace_id": ws}),
    )
    .await;
    assert!(back["archived_at"].is_null());
    let listed = workspace(&svc, serde_json::json!({"action": "list"})).await;
    assert_eq!(listed["workspaces"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn radar_dismissal_actions_and_include_dismissed() {
    let (svc, _dir) = service().await;
    let memory = svc.clone();
    let created = workspace(
        &svc,
        serde_json::json!({"action": "create", "name": "ws", "description": "d"}),
    )
    .await;
    let ws = created["workspace_id"].as_str().unwrap().to_string();

    let dismissed = workspace(
        &svc,
        serde_json::json!({"action": "dismiss_radar_item", "workspace_id": ws,
                           "kind": "open_question", "key": "Shard by tenant?"}),
    )
    .await;
    assert_eq!(
        dismissed["author"], "claude-code",
        "the connection's author"
    );
    assert!(
        crate::tools::handlers::workspace::handle(
            &svc,
            serde_json::json!({"action": "dismiss_radar_item", "workspace_id": ws,
                                          "kind": "nope", "key": "x"}),
            None,
            None,
        )
        .await
        .is_err()
    );
    let listed = workspace(
        &svc,
        serde_json::json!({"action": "list_radar_dismissals", "workspace_id": ws}),
    )
    .await;
    assert_eq!(listed["dismissals"][0]["kind"], "open_question");

    // The radar view of the workspace takes include_dismissed.
    let sid = memory.sessions.create(None, None).await.unwrap();
    for include in ["false", "true"] {
        let view = data(
            crate::tools::handlers::query::handle(
                &memory,
                serde_json::json!({
                    "session_id": sid.to_string(), "query_type": "radar",
                    "parameters": {"workspace_id": ws, "include_dismissed": include},
                }),
                None,
            )
            .await
            .unwrap(),
        );
        assert!(view["dismissed"].is_array(), "{view}");
    }

    let restored = workspace(
        &svc,
        serde_json::json!({"action": "restore_radar_item", "workspace_id": ws,
                           "kind": "open_question", "key": "Shard by tenant?"}),
    )
    .await;
    assert_eq!(restored["existed"], true);
    let listed = workspace(
        &svc,
        serde_json::json!({"action": "list_radar_dismissals", "workspace_id": ws}),
    )
    .await;
    assert!(listed["dismissals"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn session_actions_delete_messages() {
    let (svc, _dir) = service().await;
    let memory = svc.clone();
    let sid = memory.sessions.create(None, None).await.unwrap();
    let turns = ["a", "b"]
        .map(|t| NewMessage {
            turn_id: t.into(),
            role: MessageRole::Assistant,
            author: None,
            source_session: None,
            timestamp: chrono::Utc::now(),
            text: t.into(),
            tools: vec![],
            files: vec![],
            parent: None,
            blocks: Vec::new(),
        })
        .to_vec();
    memory
        .messages
        .append(post_cortex_core::services::AppendMessagesRequest {
            session_id: sid,
            messages: turns,
        })
        .await
        .unwrap();

    let call = |body: serde_json::Value| {
        let svc = svc.clone();
        async move {
            crate::tools::handlers::session::handle(&svc, body, None)
                .await
                .map(data)
        }
    };
    let one = call(
        serde_json::json!({"action": "delete_message", "session_id": sid.to_string(), "seq": 1}),
    )
    .await
    .unwrap();
    assert_eq!(
        (one["deleted"].as_u64(), one["remaining"].as_u64()),
        (Some(1), Some(1))
    );
    assert!(
        call(serde_json::json!({"action": "delete_message", "session_id": sid.to_string()}))
            .await
            .is_err(),
        "seq is required"
    );
    let all = call(serde_json::json!({"action": "delete_messages", "session_id": sid.to_string()}))
        .await
        .unwrap();
    assert_eq!(
        (all["deleted"].as_u64(), all["remaining"].as_u64()),
        (Some(1), Some(0))
    );
    assert!(
        call(serde_json::json!({"action": "delete_messages",
                                "session_id": uuid::Uuid::new_v4().to_string()}))
        .await
        .is_err(),
        "unknown session"
    );
}
