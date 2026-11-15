// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Two memory systems in one process, each served by its own MCP endpoint
//! (`rmcp_server::build_router`), stay isolated: what one tenant writes over
//! MCP is not visible to the other, for writes, global search and listings.
use crate::daemon::test_client::{Client, policy};
use axum::Router;
use post_cortex_memory::{Memory, SystemConfig};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// `semantic_search` in global scope; returns the hits' session ids.
async fn search_global(client: &mut Client, query: &str) -> Vec<String> {
    let text = client
        .call(
            "semantic_search",
            serde_json::json!({"query": query, "scope": "global", "limit": 20}),
        )
        .await;
    let Some((_, json)) = text.split_once("Results:") else {
        return Vec::new();
    };
    let data: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
    data["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["session_id"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn base_config() -> SystemConfig {
    SystemConfig {
        enable_embeddings: true,
        // Vectorized explicitly through the admin tool below.
        auto_vectorize_on_update: false,
        ..Default::default()
    }
}

/// One tenant: its memory system and the MCP router serving it.
async fn tenant(memory: Memory, ct: &CancellationToken) -> (Arc<Memory>, Router) {
    let memory = Arc::new(memory);
    let router = crate::daemon::test_client::router(memory.clone(), &policy(), None, ct).await;
    (memory, router)
}

/// Writes in A over MCP, then checks B sees nothing of it (and the reverse).
async fn assert_isolated(a: (Arc<Memory>, Router), b: (Arc<Memory>, Router)) {
    let (memory_a, router_a) = a;
    let (memory_b, router_b) = b;
    let mut client_a = Client::connect(router_a).await;
    let mut client_b = Client::connect(router_b).await;

    let created = client_a
        .call(
            "session",
            serde_json::json!({"action": "create", "name": "tenant-a"}),
        )
        .await;
    let session_a = created
        .split_whitespace()
        .find_map(|w| Uuid::parse_str(w).ok())
        .unwrap_or_else(|| panic!("no session id in {created}"));
    client_a
        .call(
            "update_conversation_context",
            serde_json::json!({
                "session_id": session_a.to_string(),
                "interaction_type": "decision_made",
                "content": {"decision": "Store the billing ledger in RocksDB",
                            "rationale": "an embedded ledger needs no database server"},
                "entities": [{"name": "billing ledger", "entity_type": "concept"},
                             {"name": "RocksDB", "entity_type": "technology"}],
                "relations": [{"from_entity": "billing ledger", "to_entity": "RocksDB",
                               "relation_type": "depends_on", "context": "storage"}],
            }),
        )
        .await;
    client_a
        .call(
            "admin",
            serde_json::json!({"action": "vectorize_session", "session_id": session_a.to_string()}),
        )
        .await;

    // The write landed in A only.
    let in_a = memory_a.sessions.get(session_a).await.unwrap();
    assert_eq!(in_a.records().len(), 1);
    assert!(memory_b.sessions.list_summaries().await.unwrap().is_empty());

    // Global search: A finds its record, B finds nothing.
    let query = "billing ledger storage RocksDB";
    let hits_a = search_global(&mut client_a, query).await;
    assert!(
        hits_a.contains(&session_a.to_string()),
        "A must find its own record: {hits_a:?}"
    );
    let hits_b = search_global(&mut client_b, query).await;
    assert!(hits_b.is_empty(), "B must not see A's record: {hits_b:?}");

    // Listings over MCP: B lists neither A's session nor its workspaces.
    let sessions_b = client_b
        .call("session", serde_json::json!({"action": "list"}))
        .await;
    assert!(!sessions_b.contains(&session_a.to_string()), "{sessions_b}");
    client_a
        .call(
            "manage_workspace",
            serde_json::json!({"action": "create", "name": "ws-a", "description": "tenant A"}),
        )
        .await;
    let workspaces_b = client_b
        .call("manage_workspace", serde_json::json!({"action": "list"}))
        .await;
    assert!(!workspaces_b.contains("ws-a"), "{workspaces_b}");
    assert!(memory_b.workspaces.list(true).is_empty());

    // The reverse: a write in B stays in B.
    let created = client_b
        .call(
            "session",
            serde_json::json!({"action": "create", "name": "tenant-b"}),
        )
        .await;
    let session_b = created
        .split_whitespace()
        .find_map(|w| Uuid::parse_str(w).ok())
        .unwrap();
    client_b
        .call(
            "update_conversation_context",
            serde_json::json!({
                "session_id": session_b.to_string(),
                "interaction_type": "decision_made",
                "content": {"decision": "Store the billing ledger in SurrealDB",
                            "rationale": "tenant B prefers a server"},
                "entities": [{"name": "billing ledger", "entity_type": "concept"},
                             {"name": "SurrealDB", "entity_type": "technology"}],
                "relations": [{"from_entity": "billing ledger", "to_entity": "SurrealDB",
                               "relation_type": "depends_on", "context": "storage"}],
            }),
        )
        .await;
    client_b
        .call(
            "admin",
            serde_json::json!({"action": "vectorize_session", "session_id": session_b.to_string()}),
        )
        .await;
    let hits_a = search_global(&mut client_a, query).await;
    assert!(
        !hits_a.contains(&session_b.to_string()),
        "A must not see B's record: {hits_a:?}"
    );
    let hits_b = search_global(&mut client_b, query).await;
    assert!(hits_b.contains(&session_b.to_string()), "{hits_b:?}");
    assert!(!hits_b.contains(&session_a.to_string()), "{hits_b:?}");
    assert_eq!(memory_a.sessions.list_summaries().await.unwrap().len(), 1);
}

#[tokio::test]
async fn two_rocksdb_tenants_in_one_process_stay_isolated() {
    let ct = CancellationToken::new();
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let at = |dir| crate::memory_tests::rocksdb_memory(dir, base_config());
    let a = tenant(at(&dir_a).await.unwrap(), &ct).await;
    let b = tenant(at(&dir_b).await.unwrap(), &ct).await;
    assert_isolated(a, b).await;
    ct.cancel();
}

#[cfg(feature = "surrealdb-storage")]
#[tokio::test]
async fn two_surrealdb_tenants_in_one_process_stay_isolated() {
    let ct = CancellationToken::new();
    let on = |namespace: &str| crate::compose::StorageConfig::SurrealDb {
        endpoint: "mem://".to_string(),
        username: None,
        password: None,
        namespace: Some(namespace.to_string()),
        database: Some("main".to_string()),
    };
    let models = crate::compose::Models::new(&crate::compose::ModelConfig::default());
    let open = |storage| {
        let models = &models;
        async move {
            crate::compose::open_memory(
                &storage,
                models,
                None,
                base_config(),
                post_cortex_memory::PipelineConfig::default(),
            )
            .await
            .unwrap()
        }
    };
    let a = tenant(open(on("tenant_a")).await, &ct).await;
    let b = tenant(open(on("tenant_b")).await, &ct).await;
    assert_isolated(a, b).await;
    ct.cancel();
}
