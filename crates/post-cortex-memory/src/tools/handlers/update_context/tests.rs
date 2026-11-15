// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::Memory;
use std::sync::Arc;
use uuid::Uuid;

async fn memory() -> (Arc<Memory>, tempfile::TempDir) {
    let (memory, dir) = crate::capabilities::test_support::rocksdb_memory().await;
    (Arc::new(memory), dir)
}

fn params(session: Uuid, author: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "session_id": session.to_string(),
        "interaction_type": "decision_made",
        "content": {"decision": "Use cards", "rationale": "always in front"},
        "entities": [{"name": "cards", "entity_type": "concept"},
                     {"name": "sessions", "entity_type": "concept"}],
        "relations": [{"from_entity": "cards", "to_entity": "sessions",
                       "relation_type": "related_to", "context": "injected"}],
    });
    if let Some(author) = author {
        body["author"] = serde_json::Value::String(author.to_string());
    }
    body
}

/// The author is the caller's own name, else the MCP client's.
#[tokio::test]
async fn author_defaults_to_the_client_name() {
    let (service, _dir) = memory().await;
    let memory = service.clone();
    let sid = memory.sessions.create(None, None).await.unwrap();
    handle(&service, params(sid, None), Some("claude-code".to_string()))
        .await
        .unwrap();
    handle(
        &service,
        params(sid, Some("julius")),
        Some("claude-code".to_string()),
    )
    .await
    .unwrap();
    handle(&service, params(sid, None), None).await.unwrap();

    let session = memory.sessions.get(sid).await.unwrap();
    let authors: Vec<Option<&str>> = session
        .records()
        .iter()
        .map(|u| u.author.as_deref())
        .collect();
    assert_eq!(authors, [Some("claude-code"), Some("julius"), None]);
}

/// A dry run runs the same checks as a real write, through Records: a
/// relation to an undeclared entity (which the old dry run passed) and an
/// unknown session are refused, a valid write is accepted, and nothing is
/// stored in any case.
#[tokio::test]
async fn dry_run_validates_like_a_write_and_stores_nothing() {
    let (service, _dir) = memory().await;
    let sid = service.sessions.create(None, None).await.unwrap();
    let dry = |mut p: serde_json::Value| {
        p["dry_run"] = serde_json::Value::Bool(true);
        p
    };

    let mut dangling = params(sid, None);
    dangling["relations"][0]["to_entity"] = "nowhere".into();
    let err = handle(&service, dry(dangling), None).await.unwrap_err();
    assert!(err.message.contains("nowhere"), "{err:?}");

    let ghost = handle(&service, dry(params(Uuid::new_v4(), None)), None)
        .await
        .unwrap_err();
    assert!(ghost.message.contains("Session not found"), "{ghost:?}");

    handle(&service, dry(params(sid, None)), None)
        .await
        .unwrap();
    let mut batch = params(sid, None);
    let item = serde_json::json!({
        "interaction_type": batch["interaction_type"].clone(),
        "content": batch["content"].clone(),
        "entities": batch["entities"].clone(),
        "relations": batch["relations"].clone(),
    });
    batch["updates"] = serde_json::json!([item.clone(), item]);
    handle(&service, dry(batch.clone()), None).await.unwrap();
    batch["updates"][1]["relations"][0]["from_entity"] = "nowhere".into();
    let err = handle(&service, dry(batch), None).await.unwrap_err();
    assert!(err.message.contains("updates[1]"), "{err:?}");

    let session = service.sessions.get(sid).await.unwrap();
    assert!(session.records().is_empty(), "a dry run stores nothing");
}
