// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The daemon's HTTP app (`rmcp_server::build_router`) end to end: health,
//! MCP initialize and tools over the streamable HTTP transport, concurrent
//! clients, and the RocksDB lock a running daemon holds on its data
//! directory (`start_rmcp_daemon`).

use crate::daemon::DaemonConfig;
use crate::daemon::bootstrap::start_rmcp_daemon;
use crate::daemon::test_client::{Client, policy};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use post_cortex_memory::SystemConfig;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

/// A router over a fresh RocksDB memory system in a temporary directory.
async fn app() -> (Router, TempDir, CancellationToken) {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(&dir, SystemConfig::default())
            .await
            .unwrap(),
    );
    let ct = CancellationToken::new();
    let router = crate::daemon::test_client::router(memory, &policy(), None, &ct).await;
    (router, dir, ct)
}

async fn health(router: &Router) -> (StatusCode, serde_json::Value) {
    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Creates a session over MCP and returns its id.
async fn create_session(client: &mut Client, name: &str) -> Uuid {
    let created = client
        .call(
            "session",
            serde_json::json!({"action": "create", "name": name, "description": "router test"}),
        )
        .await;
    created
        .split_whitespace()
        .find_map(|w| Uuid::parse_str(w).ok())
        .unwrap_or_else(|| panic!("no session id in {created}"))
}

/// REST `/health` is the one health payload (MCP `admin health` reports
/// the same fields), with `total_requests` named as what it counts and the
/// legacy `total_updates` key carrying the same count.
#[tokio::test]
async fn health_reports_ok() {
    let (router, _dir, ct) = app().await;
    let mut client = Client::connect(router.clone()).await;
    create_session(&mut client, "counted").await;
    let (status, body) = health(&router).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "post-cortex");
    assert_eq!(body["healthy"], true);
    assert_eq!(body["total_requests"], 1, "{body}");
    assert_eq!(body["total_updates"], body["total_requests"]);
    // The host's modules: the session was created through the memory
    // module's `session` tool.
    let memory = &body["modules"][0];
    assert_eq!(memory["module"], "post-cortex.memory", "{body}");
    assert_eq!(memory["calls"], 1);
    assert_eq!(memory["breaker_open"], false);
    let admin = client
        .call("admin", serde_json::json!({"action": "health"}))
        .await;
    for key in [
        "total_requests",
        "total_updates",
        "active_sessions",
        "pipeline_backlog",
    ] {
        assert!(admin.contains(key), "{key} not in {admin}");
    }
    ct.cancel();
}

#[tokio::test]
async fn initialize_returns_server_info() {
    let (router, _dir, _ct) = app().await;
    let client = Client::connect(router).await;
    assert!(client.server["serverInfo"].is_object(), "{}", client.server);
    assert!(client.server["capabilities"]["tools"].is_object());
}

/// The published tool list is `tools/list` as clients have always seen it:
/// the golden file is the list of 0.5.0 (names, descriptions, argument
/// schemas) with `manage_document` (W14) and `manage_messages` (W18), now
/// listed by the host from the memory module. Regenerate it with
/// `PCX_WRITE_GOLDEN=1`.
#[tokio::test]
async fn the_tool_list_is_the_published_one() {
    let (router, _dir, _ct) = app().await;
    let mut client = Client::connect(router).await;
    let listed = client.list_tools().await;
    if std::env::var_os("PCX_WRITE_GOLDEN").is_some() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/daemon/tools_list.json");
        let text = serde_json::to_string_pretty(&listed).unwrap();
        std::fs::write(path, text + "\n").unwrap();
    }
    let golden: serde_json::Value = serde_json::from_str(include_str!("tools_list.json")).unwrap();
    assert_eq!(listed, golden);
}

#[tokio::test]
async fn tools_list_describes_the_session_tool() {
    let (router, _dir, _ct) = app().await;
    let mut client = Client::connect(router).await;
    let tools = client.list_tools().await;
    let tool = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "session")
        .unwrap_or_else(|| panic!("no session tool in {tools}"))
        .clone();
    assert!(tool["description"].is_string());
    assert!(tool["inputSchema"].is_object());
}

#[tokio::test]
async fn concurrent_clients_do_not_block_each_other() {
    let (router, _dir, _ct) = app().await;
    let tasks: Vec<_> = (0..50)
        .map(|_| {
            let router = router.clone();
            tokio::spawn(async move {
                assert_eq!(health(&router).await.0, StatusCode::OK);
                let mut client = Client::connect(router).await;
                for _ in 0..5 {
                    assert!(client.list_tools().await["tools"].is_array());
                }
            })
        })
        .collect();
    for task in tasks {
        timeout(Duration::from_secs(30), task)
            .await
            .expect("client timed out - possible deadlock")
            .unwrap();
    }
}

#[tokio::test]
async fn concurrent_session_creates_all_succeed() {
    let (router, _dir, _ct) = app().await;
    let tasks: Vec<_> = (0..10)
        .map(|i| {
            let router = router.clone();
            tokio::spawn(async move {
                let mut client = Client::connect(router).await;
                create_session(&mut client, &format!("session {i}")).await
            })
        })
        .collect();
    let mut ids = Vec::new();
    for task in futures::future::join_all(tasks).await {
        ids.push(task.unwrap());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 10);

    let mut client = Client::connect(router).await;
    let listed = client
        .call("session", serde_json::json!({"action": "list"}))
        .await;
    for id in ids {
        assert!(listed.contains(&id.to_string()), "{id} not in {listed}");
    }
}

#[tokio::test]
async fn a_written_record_is_found_by_session_search() {
    let (router, _dir, _ct) = app().await;
    let mut client = Client::connect(router).await;
    let session = create_session(&mut client, "daemon mode").await;
    client
        .call(
            "update_conversation_context",
            serde_json::json!({
                "session_id": session.to_string(),
                "interaction_type": "qa",
                "content": {"question": "How does daemon mode work?",
                            "answer": "Several clients share one RocksDB through the daemon"},
                "entities": [{"name": "daemon mode", "entity_type": "concept"},
                             {"name": "RocksDB", "entity_type": "technology"}],
                "relations": [{"from_entity": "daemon mode", "to_entity": "RocksDB",
                               "relation_type": "depends_on", "context": "shared storage"}],
            }),
        )
        .await;
    {
        client
            .call(
                "admin",
                serde_json::json!({"action": "vectorize_session", "session_id": session.to_string()}),
            )
            .await;
        let found = client
            .call(
                "semantic_search",
                serde_json::json!({"query": "daemon mode shared RocksDB", "scope": "session",
                                   "scope_id": session.to_string(), "limit": 10}),
            )
            .await;
        assert!(found.contains("daemon mode"), "{found}");
    }
}

#[tokio::test]
async fn a_running_daemon_holds_the_rocksdb_lock() {
    let dir = tempfile::tempdir().unwrap();
    let port = portpicker::pick_unused_port().expect("a free port");
    let config = DaemonConfig {
        host: "127.0.0.1".to_string(),
        port,
        grpc_port: 0,
        data_directory: dir.path().to_path_buf(),
        storage_backend: "rocksdb".to_string(),
        ..Default::default()
    };
    let daemon = tokio::spawn(start_rmcp_daemon(config));

    // One client for every look, which reads no system proxy: on macOS that
    // lookup lists the directory of the test's binary, synchronously on this
    // runtime's only thread, and target/debug/deps can hold enough files
    // that it outlasts the wait.
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("an HTTP client");
    let url = format!("http://127.0.0.1:{port}/health");
    let up = timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(response) = client.get(&url).send().await
                && response.status().is_success()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(up.is_ok(), "daemon did not come up on port {port}");

    let second = crate::memory_tests::rocksdb_memory(&dir, SystemConfig::default()).await;
    let error = second
        .err()
        .expect("a second memory system on the same directory must fail");
    let lower = error.to_string().to_lowercase();
    assert!(
        lower.contains("lock") || lower.contains("io error"),
        "expected a lock error, got: {error}"
    );
    daemon.abort();
}
