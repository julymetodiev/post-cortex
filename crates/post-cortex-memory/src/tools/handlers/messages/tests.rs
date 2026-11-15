// Copyright (c) 2026 Julius ML
// MIT License

//! `manage_messages`: an agent's turns appended whole and paged back.

use super::*;

use serde_json::json;

#[tokio::test]
async fn an_agents_turns_are_appended_whole_and_paged_back() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let session = memory.sessions.create(None, None).await.unwrap();
    let s = session.to_string();
    let turns = json!([
        { "turn_id": "u1", "role": "user", "text": "What is in README.md?" },
        { "turn_id": "a1", "role": "assistant", "parent": "u1", "tools": ["fs_read"],
          "blocks": [{ "type": "tool_use", "id": "c1", "name": "fs_read", "input": { "path": "README.md" } }] },
        { "turn_id": "u2", "role": "user", "parent": "a1",
          "blocks": [{ "type": "tool_result", "tool_use_id": "c1", "content": "# pcx", "is_error": false }] },
        { "turn_id": "a2", "role": "assistant", "parent": "u2", "text": "A title: pcx." }
    ]);
    let appended = handle(
        &memory,
        json!({ "action": "append", "session_id": s, "messages": turns }),
        Some("pcx-agent".into()),
    )
    .await
    .unwrap()
    .data
    .unwrap();
    assert_eq!(appended["appended"], json!([0, 1, 2, 3]));

    let first = handle(
        &memory,
        json!({ "action": "page", "session_id": s, "limit": 2 }),
        None,
    )
    .await
    .unwrap()
    .data
    .unwrap();
    assert_eq!(first["next_from"], json!(2));
    let call = &first["messages"][1];
    assert_eq!(call["parent"], json!("u1"));
    assert_eq!(call["author"], json!("pcx-agent"));
    assert_eq!(call["blocks"][0]["input"], json!({ "path": "README.md" }));
    let rest = handle(
        &memory,
        json!({ "action": "page", "session_id": s, "from": 2, "limit": 2 }),
        None,
    )
    .await
    .unwrap()
    .data
    .unwrap();
    assert_eq!(
        rest["messages"][0]["blocks"][0]["type"],
        json!("tool_result")
    );
    assert_eq!(rest["messages"][1]["text"], json!("A title: pcx."));

    let bad = handle(
        &memory,
        json!({ "action": "append", "session_id": s, "messages": [
            { "turn_id": "x", "role": "assistant", "blocks": [{ "type": "tool_use", "id": "c" }] }
        ] }),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(bad.kind, crate::tools::Kind::InvalidArgument);
    assert!(
        bad.message.contains("messages[0].blocks[0]"),
        "{}",
        bad.message
    );
}
