// Integration tests for daemon HTTP server with in-memory testing
// Tests are run serially to avoid race conditions with shared resources
mod helpers;

use helpers::TestApp;
use hyper::StatusCode;
use post_cortex::daemon::{DaemonConfig, DaemonServer};
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

// Use serial_test to prevent race conditions
use serial_test::serial;

/// Setup test app without TCP server
async fn setup_test_app() -> (TestApp, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    let config = DaemonConfig {
        host: "127.0.0.1".to_string(),
        port: 0, // Unused in testing
        data_directory: temp_dir.path().to_str().unwrap().to_string(),
        storage_backend: "surrealdb".to_string(),
        surrealdb_endpoint: Some("ws://localhost:8000".to_string()),
        surrealdb_username: Some("root".to_string()),
        surrealdb_password: Some("root".to_string()),
        surrealdb_namespace: "post_cortex".to_string(),
        surrealdb_database: "main".to_string(),
    };

    let server = DaemonServer::new(config).await.unwrap();
    let router = server.build_router();
    let app = TestApp::new(router);

    (app, temp_dir)
}

/// Setup test daemon with real TCP for specific tests (e.g., RocksDB lock test)
async fn start_real_daemon() -> (u16, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    // Find free port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = DaemonConfig {
        host: "127.0.0.1".to_string(),
        port,
        data_directory: temp_dir.path().to_str().unwrap().to_string(),
        storage_backend: "surrealdb".to_string(),
        surrealdb_endpoint: Some("ws://localhost:8000".to_string()),
        surrealdb_username: Some("root".to_string()),
        surrealdb_password: Some("root".to_string()),
        surrealdb_namespace: "post_cortex".to_string(),
        surrealdb_database: "main".to_string(),
    };

    let server = DaemonServer::new(config).await.unwrap();

    // Start server in background
    tokio::spawn(async move {
        server.start().await.unwrap();
    });

    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    (port, temp_dir)
}

#[serial]
#[tokio::test]
async fn test_daemon_health_check() {
    let (app, _temp_dir) = setup_test_app().await;

    let response = app.get("/health").await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = TestApp::json_body(response).await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "post-cortex-daemon");
}

#[serial]
#[tokio::test]
async fn test_daemon_stats_endpoint() {
    let (app, _temp_dir) = setup_test_app().await;

    let response = app.get("/stats").await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = TestApp::json_body(response).await;
    assert!(body["active_connections"].is_number());
    assert!(body["total_requests"].is_number());
    assert!(body["workspace_count"].is_number());
}

#[serial]
#[tokio::test]
async fn test_daemon_mcp_initialize() {
    let (app, _temp_dir) = setup_test_app().await;

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });

    let response = app.post_json("/message", request).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = TestApp::json_body(response).await;
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    assert!(body["result"].is_object());
    assert!(body["result"]["serverInfo"].is_object());
}

#[serial]
#[tokio::test]
async fn test_multiple_concurrent_clients() {
    let (app, _temp_dir) = setup_test_app().await;

    // Spawn 10 concurrent requests
    let tasks: Vec<_> = (0..10)
        .map(|i| {
            let app_clone = TestApp::new(app.router.clone());
            tokio::spawn(async move {
                // Make health check request
                let health_response = app_clone.get("/health").await;
                assert_eq!(health_response.status(), StatusCode::OK);

                // Make MCP initialize request
                let mcp_request = json!({
                    "jsonrpc": "2.0",
                    "id": i,
                    "method": "initialize",
                    "params": {}
                });

                let mcp_response = app_clone.post_json("/message", mcp_request).await;

                assert_eq!(mcp_response.status(), StatusCode::OK);
                let body = TestApp::json_body(mcp_response).await;
                assert_eq!(body["id"], i);
            })
        })
        .collect();

    // Wait for all clients - if deadlock, this will timeout
    for task in tasks {
        timeout(Duration::from_secs(5), task)
            .await
            .expect("Task timed out - possible deadlock")
            .unwrap();
    }

    // Verify stats increased
    let response = app.get("/stats").await;
    let body = TestApp::json_body(response).await;
    assert!(body["total_requests"].as_u64().unwrap() >= 10);
}

#[serial]
#[tokio::test]
async fn test_stress_concurrent_requests() {
    let (app, _temp_dir) = setup_test_app().await;

    // Spawn 50 concurrent clients making multiple requests each
    let tasks: Vec<_> = (0..50)
        .map(|i| {
            let app_clone = TestApp::new(app.router.clone());
            tokio::spawn(async move {
                // Each client makes 5 requests
                for j in 0..5 {
                    let request = json!({
                        "jsonrpc": "2.0",
                        "id": format!("{}-{}", i, j),
                        "method": "initialize",
                        "params": {}
                    });

                    let response = app_clone.post_json("/message", request).await;
                    assert_eq!(response.status(), StatusCode::OK);
                }
            })
        })
        .collect();

    // Wait for all - 250 total requests
    for task in tasks {
        timeout(Duration::from_secs(10), task)
            .await
            .expect("Stress test timed out - possible deadlock")
            .unwrap();
    }

    // Verify total requests
    let response = app.get("/stats").await;
    let body = TestApp::json_body(response).await;
    assert!(body["total_requests"].as_u64().unwrap() >= 250);
}

#[serial]
#[tokio::test]
async fn test_create_session_tool() {
    let (app, _temp_dir) = setup_test_app().await;

    // Call create_session tool
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "create_session",
            "arguments": {
                "name": "Test Session",
                "description": "Integration test session"
            }
        }
    });

    let response = app.post_json("/message", request).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = TestApp::json_body(response).await;

    // Verify response structure
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    assert!(body["result"].is_object());
    assert!(body["result"]["content"].is_array());

    // Verify session was created
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Created new session"));
    assert!(text.contains("-")); // UUID contains dashes
}

#[serial]
#[tokio::test]
async fn test_tools_list_includes_create_session() {
    let (app, _temp_dir) = setup_test_app().await;

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    let response = app.post_json("/message", request).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = TestApp::json_body(response).await;
    assert!(body["result"]["tools"].is_array());

    let tools = body["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());

    let create_session_tool = tools.iter().find(|t| t["name"] == "create_session");

    assert!(create_session_tool.is_some());
    let tool = create_session_tool.unwrap();
    assert!(tool["description"].is_string());
    assert!(tool["inputSchema"].is_object());
}

#[serial]
#[tokio::test]
async fn test_concurrent_create_sessions() {
    let (app, _temp_dir) = setup_test_app().await;

    // Create 10 sessions concurrently
    let tasks: Vec<_> = (0..10)
        .map(|i| {
            let app_clone = TestApp::new(app.router.clone());
            tokio::spawn(async move {
                let request = json!({
                    "jsonrpc": "2.0",
                    "id": i,
                    "method": "tools/call",
                    "params": {
                        "name": "create_session",
                        "arguments": {
                            "name": format!("Session {}", i),
                            "description": format!("Concurrent test {}", i)
                        }
                    }
                });

                let response = app_clone.post_json("/message", request).await;

                assert_eq!(response.status(), StatusCode::OK);

                let body = TestApp::json_body(response).await;
                assert!(body["result"].is_object());

                body["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        })
        .collect();

    // Wait for all sessions to be created
    let results = futures::future::join_all(tasks).await;

    // Verify all succeeded
    assert_eq!(results.len(), 10);
    for result in results {
        let text = result.unwrap();
        assert!(text.contains("Created new session"));
    }
}

#[serial]
#[tokio::test]
async fn test_daemon_shares_rocksdb() {
    // This test MUST use real TCP to verify RocksDB locking
    let (port, temp_dir) = start_real_daemon().await;

    // Verify server is running
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());

    // Try to create second ConversationMemorySystem instance with same data dir
    // This should FAIL because daemon already has RocksDB open
    let config = post_cortex::SystemConfig {
        data_directory: temp_dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let result = post_cortex::ConversationMemorySystem::new(config).await;

    // Should fail with lock error
    assert!(result.is_err());
    if let Err(error_msg) = result {
        let error_lower = error_msg.to_lowercase();
        assert!(
            error_lower.contains("lock") || error_lower.contains("io error"),
            "Expected lock error, got: {}",
            error_msg
        );
    }
}

#[serial]
#[tokio::test]
async fn test_update_conversation_context_tool() {
    let (app, _temp_dir) = setup_test_app().await;

    // First create a session
    let create_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "create_session",
            "arguments": {
                "name": "Test Session",
                "description": "Testing update_conversation_context"
            }
        }
    });

    let response = app.post_json("/message", create_request).await;

    let body = TestApp::json_body(response).await;
    let session_text = body["result"]["content"][0]["text"].as_str().unwrap();
    let session_id = session_text
        .split("Created new session: ")
        .nth(1)
        .unwrap()
        .trim();

    // Now update context
    let update_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "update_conversation_context",
            "arguments": {
                "session_id": session_id,
                "interaction_type": "qa",
                "content": {
                    "question": "How does daemon mode work?",
                    "answer": "Daemon mode allows multiple Claude instances to share RocksDB"
                }
            }
        }
    });

    let response = app.post_json("/message", update_request).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = TestApp::json_body(response).await;
    assert!(body["result"].is_object());
}

#[serial]
#[tokio::test]
async fn test_semantic_search_session_tool() {
    let (app, _temp_dir) = setup_test_app().await;

    // Create session and add context
    let create_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "create_session",
            "arguments": {}
        }
    });

    let response = app.post_json("/message", create_request).await;

    let body = TestApp::json_body(response).await;
    let session_text = body["result"]["content"][0]["text"].as_str().unwrap();
    let session_id = session_text
        .split("Created new session: ")
        .nth(1)
        .unwrap()
        .trim();

    // Search in session
    let search_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "semantic_search_session",
            "arguments": {
                "session_id": session_id,
                "query": "daemon mode",
                "limit": 10
            }
        }
    });

    let response = app.post_json("/message", search_request).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = TestApp::json_body(response).await;
    assert!(body["result"].is_object());
}

#[serial]
#[tokio::test]
async fn test_list_sessions_tool() {
    let (app, _temp_dir) = setup_test_app().await;

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "list_sessions",
            "arguments": {}
        }
    });

    let response = app.post_json("/message", request).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = TestApp::json_body(response).await;
    assert!(body["result"].is_object());
}

#[serial]
#[tokio::test]
async fn test_list_sessions_debug() {
    let (app, _temp_dir) = setup_test_app().await;

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "list_sessions",
            "arguments": {}
        }
    });

    let response = app.post_json("/message", request).await;

    let body = TestApp::json_body(response).await;
    println!(
        "Response body: {}",
        serde_json::to_string_pretty(&body).unwrap()
    );
}
