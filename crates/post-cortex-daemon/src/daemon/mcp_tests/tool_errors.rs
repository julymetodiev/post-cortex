// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Failed MCP tool calls reach the client as tool errors (`isError: true`)
//! with the error text: rejected writes, partial bulk failures and failures
//! a handler raises as a JSON-RPC error.
use crate::daemon::test_client::{Client, policy, text_of};
use post_cortex_memory::{Memory, SystemConfig};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Fixture {
    client: Client,
    memory: Arc<Memory>,
    _dir: tempfile::TempDir,
    ct: CancellationToken,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ct = CancellationToken::new();
    let router = crate::daemon::test_client::router(memory.clone(), &policy(), None, &ct).await;
    Fixture {
        client: Client::connect(router).await,
        memory,
        _dir: dir,
        ct,
    }
}

fn update(interaction_type: &str, to_entity: &str) -> serde_json::Value {
    serde_json::json!({
        "interaction_type": interaction_type,
        "content": {"decision": "Use cards", "rationale": "always in front"},
        "entities": [{"name": "cards", "entity_type": "concept"},
                     {"name": "sessions", "entity_type": "concept"}],
        "relations": [{"from_entity": "cards", "to_entity": to_entity,
                       "relation_type": "related_to", "context": "injected"}],
    })
}

fn assert_tool_error(message: &serde_json::Value, needle: &str) {
    assert!(
        message.get("error").is_none(),
        "a tool error, not a JSON-RPC error: {message}"
    );
    assert_eq!(
        message["result"]["isError"],
        serde_json::Value::Bool(true),
        "{message}"
    );
    let text = text_of(message);
    assert!(text.contains(needle), "{needle:?} not in {text:?}");
}

#[tokio::test]
async fn a_rejected_write_is_a_tool_error_with_its_reason() {
    let mut f = fixture().await;
    let sid = f.memory.sessions.create(None, None).await.unwrap();
    let mut body = update("decision_made", "nowhere");
    body["session_id"] = serde_json::json!(sid.to_string());

    let message = f.client.call_raw("update_conversation_context", body).await;

    assert_tool_error(&message, "is not declared in the entities list");
    f.ct.cancel();
}

/// A bulk write is atomic: a malformed item (an unknown type) fails the
/// whole batch, nothing is written, and the tool error carries one result
/// per input item in input order.
#[tokio::test]
async fn a_bad_bulk_item_is_a_tool_error_and_writes_nothing() {
    let mut f = fixture().await;
    let sid = f.memory.sessions.create(None, None).await.unwrap();
    let body = serde_json::json!({
        "session_id": sid.to_string(),
        "updates": [
            update("not_a_type", "sessions"),
            update("decision_made", "sessions"),
            update("decision_made", "nowhere"),
        ],
    });

    let message = f.client.call_raw("update_conversation_context", body).await;

    assert_tool_error(&message, "0 succeeded, 3 failed");
    let text = text_of(&message);
    let (_, json) = text.split_once("Results:").expect("per-item results");
    let data: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
    let results = data["results"].as_array().expect("results array");
    assert_eq!(results.len(), 3, "{data}");
    for (i, r) in results.iter().enumerate() {
        assert_eq!(r["index"], serde_json::json!(i), "input order: {data}");
    }
    assert!(
        results[0]["error"]
            .as_str()
            .unwrap()
            .contains("Unknown interaction type")
    );
    assert!(
        results[1]["error"]
            .as_str()
            .unwrap()
            .contains("not written")
    );
    let session = f.memory.sessions.get(sid).await.unwrap();
    assert!(session.records().is_empty(), "the batch wrote nothing");

    // Every item translates, one is invalid: the service refuses the batch.
    let body = serde_json::json!({
        "session_id": sid.to_string(),
        "updates": [update("decision_made", "sessions"), update("decision_made", "nowhere")],
    });
    let message = f.client.call_raw("update_conversation_context", body).await;
    assert_tool_error(&message, "0 succeeded, 2 failed");
    assert!(
        f.memory
            .sessions
            .get(sid)
            .await
            .unwrap()
            .records()
            .is_empty()
    );
    f.ct.cancel();
}

/// An invalid numeric query parameter is invalid params, never a default.
#[tokio::test]
async fn an_invalid_numeric_query_parameter_is_invalid_params() {
    let mut f = fixture().await;
    let sid = f.memory.sessions.create(None, None).await.unwrap();
    for (query_type, name) in [
        ("entity_importance", "limit"),
        ("get_most_important_entities", "limit"),
        ("trace_relationships", "max_depth"),
    ] {
        let message = f
            .client
            .call_raw(
                "query_conversation_context",
                serde_json::json!({
                    "session_id": sid.to_string(),
                    "query_type": query_type,
                    "parameters": {name: "ten", "entity_name": "x"},
                }),
            )
            .await;
        assert_tool_error(&message, "\"ten\" is not");
    }
    f.ct.cancel();
}

#[tokio::test]
async fn a_handler_failure_is_a_tool_error_with_its_text() {
    let mut f = fixture().await;
    let missing = Uuid::new_v4();

    let message = f
        .client
        .call_raw(
            "session",
            serde_json::json!({"action": "load", "session_id": missing.to_string()}),
        )
        .await;

    assert_tool_error(&message, "not found");
    f.ct.cancel();
}

#[tokio::test]
async fn an_unknown_tool_stays_a_protocol_error() {
    let mut f = fixture().await;
    let message = f
        .client
        .call_raw("no_such_tool", serde_json::json!({}))
        .await;
    assert!(message.get("error").is_some(), "{message}");
    f.ct.cancel();
}
