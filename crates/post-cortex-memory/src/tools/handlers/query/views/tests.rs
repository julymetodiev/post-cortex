// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// `query_type=assemble_context` is the assemble_context tool over the
/// session (review W4 item 9): the cards come first, as they do for the
/// tool, instead of a session-only assembly without them.
#[tokio::test]
async fn the_assemble_context_query_is_the_tool() {
    use post_cortex_core::core::card::CardScope;
    use post_cortex_core::services::UpdateCardRequest;

    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    memory
        .cards
        .update(UpdateCardRequest {
            scope: CardScope::User,
            text: "Answer briefly.".into(),
            expected_version: 0,
            author: None,
        })
        .await
        .unwrap();
    let params = [("query".to_string(), "anything".to_string())].into();
    let reply = assemble(&memory, sid, &params, None).await.unwrap();
    let data: serde_json::Value = reply.data.unwrap();
    let cards = &data["AssembledContext"]["cards"];
    assert_eq!(cards[0]["text"], "Answer briefly.", "{data}");
}
