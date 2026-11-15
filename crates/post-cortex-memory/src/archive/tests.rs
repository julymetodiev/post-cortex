// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An archived workspace takes no change but its restoring and its
//! deleting, by a call that names it or a session of it.

use serde_json::json;

use super::*;
use crate::capabilities::CreateWorkspace;
use crate::capabilities::test_support::rocksdb_memory;
use crate::tools::operations::classify::mcp;
use post_cortex_core::workspace::SessionRole;

#[tokio::test]
async fn an_archived_workspace_is_read_only_until_it_is_restored() {
    let (memory, _dir) = rocksdb_memory().await;
    let session = memory.sessions.create(None, None).await.unwrap();
    let other = memory.sessions.create(None, None).await.unwrap();
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "harbour".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    memory
        .workspaces
        .add_session(ws, session, SessionRole::Primary)
        .await
        .unwrap();
    let (sid, wid) = (session.to_string(), ws.to_string());
    let call = |tool: &str, arguments: serde_json::Value| mcp(tool, &arguments).unwrap();
    let write = call(
        "update_conversation_context",
        json!({ "session_id": sid, "interaction_type": "qa", "content": {} }),
    );
    let rename = call(
        "manage_workspace",
        json!({ "action": "update", "workspace_id": wid, "name": "port" }),
    );
    let search = call(
        "semantic_search",
        json!({ "query": "x", "scope": "session", "scope_id": sid }),
    );
    let forget = call("session", json!({ "action": "delete", "session_id": sid }));
    let elsewhere = call(
        "update_conversation_context",
        json!({ "session_id": other.to_string(), "interaction_type": "qa", "content": {} }),
    );
    assert!(write.changes() && rename.changes() && forget.changes());
    assert!(!search.changes());
    for open in [&write, &rename, &search, &forget, &elsewhere] {
        assert_eq!(memory.open_to(open), Ok(()), "{}", open.operation.name);
    }

    memory.workspaces.set_archived(ws, true).await.unwrap();
    for refused in [&write, &rename, &forget] {
        let op = refused.operation.name;
        assert_eq!(
            memory.open_to(refused),
            Err(Archived { op, workspace: ws }),
            "{op}"
        );
    }
    let told = memory.open_to(&write).unwrap_err().to_string();
    assert!(told.contains("archived and read-only"), "{told}");
    // Reading it, writing elsewhere, restoring it and deleting it go on.
    let restore = call(
        "manage_workspace",
        json!({ "action": "unarchive", "workspace_id": wid }),
    );
    let delete = call(
        "manage_workspace",
        json!({ "action": "delete", "workspace_id": wid }),
    );
    for open in [&search, &elsewhere, &restore, &delete] {
        assert_eq!(memory.open_to(open), Ok(()), "{}", open.operation.name);
    }

    memory.workspaces.set_archived(ws, false).await.unwrap();
    assert_eq!(memory.open_to(&write), Ok(()));
}
