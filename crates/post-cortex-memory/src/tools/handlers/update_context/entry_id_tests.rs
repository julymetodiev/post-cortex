// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use std::sync::Arc;

/// A write over MCP returns the id of the record it stored (for
/// `supersedes`, forget and purge) and whether it is durable.
#[tokio::test]
async fn a_write_returns_its_entry_id() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let memory = Arc::new(memory);
    let service = memory.clone();
    let sid = memory.sessions.create(None, None).await.unwrap();
    let body = serde_json::json!({
        "session_id": sid.to_string(),
        "interaction_type": "decision_made",
        "content": {"decision": "Use cards", "rationale": "always in front"},
        "entities": [{"name": "cards", "entity_type": "concept"},
                     {"name": "sessions", "entity_type": "concept"}],
        "relations": [{"from_entity": "cards", "to_entity": "sessions",
                       "relation_type": "related_to", "context": "injected"}],
    });

    let result = handle(&service, body, None).await.unwrap();

    let data: serde_json::Value = result.data.unwrap();
    let stored = memory.sessions.get(sid).await.unwrap();
    assert_eq!(
        data["entry_id"],
        serde_json::json!(stored.records()[0].id.to_string())
    );
    assert_eq!(data["durable"], serde_json::json!(true));
}
