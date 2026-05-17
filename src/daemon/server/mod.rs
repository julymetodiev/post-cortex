// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! Lock-free HTTP daemon server.
//!
//! Provides:
//! - MCP JSON-RPC endpoint (`/message`) + Streamable-HTTP SSE (`/sse`)
//! - REST API (`/api/*`) for the local `pcx` CLI
//! - Health (`/health`) and stats (`/stats`)
//!
//! Tool-call business logic lives in `tools::mcp::*`; this module is a thin
//! transport layer that maps JSON payloads onto those calls.

use crate::ConversationMemorySystem;
use crate::daemon::sse::SSEBroadcaster;
use axum::{
    Json, Router,
    extract::State,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
    routing::{delete, get, post},
};
use dashmap::DashMap;
use futures::stream::{self};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tower_http::cors::CorsLayer;
use tracing::{debug, info};
use uuid::Uuid;

use super::config::DaemonConfig;

mod handlers;
mod rest;

/// Connection information (lock-free)
#[allow(dead_code)] // Will be used for connection tracking in future
struct ConnectionInfo {
    id: Uuid,
    connected_at: SystemTime,
    last_request: Arc<AtomicU64>,
    request_count: Arc<AtomicU64>,
}

/// Lock-free daemon server
pub struct DaemonServer {
    /// Core memory system (already lock-free)
    pub(super) memory_system: Arc<ConversationMemorySystem>,

    /// Lock-free connection tracking
    active_connections: Arc<DashMap<Uuid, ConnectionInfo>>,

    /// Lock-free SSE broadcaster
    sse_broadcaster: Arc<SSEBroadcaster>,

    /// Map session IDs to SSE client IDs for routing responses
    session_to_client: Arc<DashMap<String, Uuid>>,

    /// Atomic metrics
    connection_counter: Arc<AtomicU64>,
    total_requests: Arc<AtomicU64>,
    config: DaemonConfig,
}

impl DaemonServer {
    pub async fn new(config: DaemonConfig) -> Result<Self, String> {
        info!(
            "Initializing lock-free daemon server on {}:{}",
            config.host, config.port
        );

        // Create memory system with storage backend from config
        #[allow(unused_mut)]
        let mut system_config = crate::SystemConfig {
            data_directory: config.data_directory.clone(),
            ..Default::default()
        };

        // Configure storage backend if surrealdb-storage feature is enabled
        #[cfg(feature = "surrealdb-storage")]
        {
            use crate::storage::traits::StorageBackendType;

            system_config.storage_backend = match config.storage_backend.as_str() {
                "surrealdb" => StorageBackendType::SurrealDB,
                _ => StorageBackendType::RocksDB,
            };
            system_config.surrealdb_endpoint = config.surrealdb_endpoint.clone();
            system_config.surrealdb_username = config.surrealdb_username.clone();
            system_config.surrealdb_password = config.surrealdb_password.clone();
            system_config.surrealdb_namespace = Some(config.surrealdb_namespace.clone());
            system_config.surrealdb_database = Some(config.surrealdb_database.clone());

            if system_config.storage_backend == StorageBackendType::SurrealDB {
                info!(
                    "Using SurrealDB storage backend: {} (ns: {}, db: {})",
                    system_config
                        .surrealdb_endpoint
                        .as_deref()
                        .unwrap_or("not configured"),
                    config.surrealdb_namespace,
                    config.surrealdb_database
                );
            } else {
                info!("Using RocksDB storage backend");
            }
        }

        let memory_system = Arc::new(
            ConversationMemorySystem::new(system_config)
                .await
                .map_err(|e| format!("Failed to initialize memory system: {}", e))?,
        );

        // Inject memory system into MCP tools global singleton
        // This enables all MCP tool functions to use daemon's shared memory system
        crate::tools::mcp::inject_memory_system(memory_system.clone());

        info!("Memory system initialized and injected successfully");

        Ok(Self {
            memory_system,
            active_connections: Arc::new(DashMap::new()),
            sse_broadcaster: Arc::new(SSEBroadcaster::new()),
            session_to_client: Arc::new(DashMap::new()),
            connection_counter: Arc::new(AtomicU64::new(0)),
            total_requests: Arc::new(AtomicU64::new(0)),
            config,
        })
    }

    /// Build Axum router for testing without starting TCP server.
    ///
    /// Exposes the router for in-memory HTTP testing, eliminating the need
    /// for real TCP ports in tests.
    pub fn build_router(self) -> Router {
        let server = Arc::new(self);

        Router::new()
            .route("/health", get(health_check))
            .route("/sse", get(handle_sse_stream))
            .route("/message", post(handle_mcp_request))
            .route("/stats", get(get_stats))
            // REST API for CLI
            .route(
                "/api/sessions",
                get(rest::api_list_sessions).post(rest::api_create_session),
            )
            .route("/api/sessions/{id}", delete(rest::api_delete_session))
            .route(
                "/api/workspaces",
                get(rest::api_list_workspaces).post(rest::api_create_workspace),
            )
            .route("/api/workspaces/{id}", delete(rest::api_delete_workspace))
            .route(
                "/api/workspaces/{workspace_id}/sessions/{session_id}",
                post(rest::api_attach_session),
            )
            .layer(CorsLayer::permissive())
            .with_state(server)
    }

    /// Start HTTP server
    pub async fn start(self) -> Result<(), String> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| format!("Invalid address: {}", e))?;

        let app = self.build_router();

        info!("Starting HTTP server on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;

        info!("HTTP server listening on {}", addr);

        axum::serve(listener, app)
            .await
            .map_err(|e| format!("Server error: {}", e))?;

        Ok(())
    }

    /// Get server statistics
    fn get_statistics(&self) -> ServerStats {
        ServerStats {
            active_connections: self.active_connections.len() as u64,
            total_connections: self.connection_counter.load(Ordering::Relaxed),
            total_requests: self.total_requests.load(Ordering::Relaxed),
            active_sse_clients: self.sse_broadcaster.active_clients(),
            total_sse_events: self.sse_broadcaster.total_events(),
            workspace_count: self.memory_system.workspace_manager.total_workspaces(),
        }
    }
}

// =============================================================================
// MCP JSON-RPC types
// =============================================================================

#[derive(Debug, Deserialize)]
struct MCPRequest {
    #[allow(dead_code)] // Used for validation
    jsonrpc: String,
    id: serde_json::Value,
    method: String,
    #[allow(dead_code)] // Will be used for tool calls
    params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct MCPResponse {
    jsonrpc: String,
    id: serde_json::Value,
    result: Option<serde_json::Value>,
    error: Option<MCPError>,
}

#[derive(Debug, Serialize)]
struct MCPError {
    code: i32,
    message: String,
}

/// Server statistics
#[derive(Debug, Serialize)]
struct ServerStats {
    active_connections: u64,
    total_connections: u64,
    total_requests: u64,
    active_sse_clients: u64,
    total_sse_events: u64,
    workspace_count: u64,
}

// =============================================================================
// Health & stats endpoints
// =============================================================================

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "post-cortex-daemon"
    }))
}

async fn get_stats(State(server): State<Arc<DaemonServer>>) -> impl IntoResponse {
    Json(server.get_statistics())
}

// =============================================================================
// SSE stream endpoint for Streamable HTTP transport
// =============================================================================

async fn handle_sse_stream(State(server): State<Arc<DaemonServer>>) -> impl IntoResponse {
    use axum::http::header::{HeaderMap, HeaderName, HeaderValue};

    let client_id = Uuid::new_v4();
    let session_id = Uuid::new_v4().to_string();
    let rx = server.sse_broadcaster.register_client(client_id);

    // Map session ID to client ID for routing POST responses
    server
        .session_to_client
        .insert(session_id.clone(), client_id);

    info!(
        "SSE stream connected: {} (session: {})",
        client_id, session_id
    );

    let stream = stream::unfold(
        (rx, client_id, session_id.clone(), server.clone(), true),
        |(mut rx, client_id, session_id, server, first)| async move {
            // Send initial endpoint event
            if first {
                let endpoint_event = Event::default()
                    .event("endpoint")
                    .id("0")
                    .json_data(serde_json::json!({"uri": "/message"}))
                    .ok()?;
                return Some((
                    Ok::<_, std::convert::Infallible>(endpoint_event),
                    (rx, client_id, session_id, server, false),
                ));
            }

            // Wait for events from broadcaster
            match rx.recv().await {
                Some(event) => {
                    let sse_event = Event::default()
                        .event(&event.event_type)
                        .id(event.id)
                        .json_data(&event.data)
                        .ok()?;
                    Some((
                        Ok::<_, std::convert::Infallible>(sse_event),
                        (rx, client_id, session_id, server, false),
                    ))
                }
                None => {
                    // Cleanup on disconnect
                    server.sse_broadcaster.unregister_client(&client_id);
                    server.session_to_client.remove(&session_id);
                    info!(
                        "SSE stream disconnected: {} (session: {})",
                        client_id, session_id
                    );
                    None
                }
            }
        },
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("mcp-session-id"),
        HeaderValue::from_str(&session_id).unwrap(),
    );

    (headers, Sse::new(stream))
}

// =============================================================================
// MCP JSON-RPC request handler
// =============================================================================

/// MCP request handler — Streamable HTTP transport (2025-03-26).
/// Returns response directly as JSON (not via SSE).
async fn handle_mcp_request(
    State(server): State<Arc<DaemonServer>>,
    Json(request): Json<MCPRequest>,
) -> impl IntoResponse {
    debug!("Handling MCP request: {}", request.method);
    server.total_requests.fetch_add(1, Ordering::Relaxed);

    // Route to appropriate handler
    let result = match request.method.as_str() {
        "initialize" => handle_initialize(),
        "tools/list" => handle_tools_list(&server),
        "tools/call" => handle_tool_call(&server, &request).await,
        _ => Err(format!("Unknown method: {}", request.method)),
    };

    // Build and return JSON-RPC response
    Json(match result {
        Ok(result_data) => MCPResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: Some(result_data),
            error: None,
        },
        Err(error_msg) => MCPResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: None,
            error: Some(MCPError {
                code: -32603,
                message: error_msg,
            }),
        },
    })
}

fn handle_initialize() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "protocolVersion": "2025-03-26",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "post-cortex-daemon",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

fn handle_tools_list(_server: &Arc<DaemonServer>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "tools": [
            {
                "name": "create_session",
                "description": "Create a new conversation session with optional name and description",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Optional name for the session"},
                        "description": {"type": "string", "description": "Optional description for the session"}
                    }
                }
            },
            {
                "name": "update_conversation_context",
                "description": "Add new interaction context to a session",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": {"type": "string", "description": "UUID of the session"},
                        "interaction_type": {"type": "string", "description": "Type: qa, code_change, problem_solved, decision_made, requirement_added, concept_defined"},
                        "content": {"type": "object", "description": "Content object with interaction data"}
                    },
                    "required": ["session_id", "interaction_type", "content"]
                }
            }
        ]
    }))
}

async fn handle_tool_call(
    server: &Arc<DaemonServer>,
    request: &MCPRequest,
) -> Result<serde_json::Value, String> {
    let params = request
        .params
        .as_ref()
        .ok_or_else(|| "Missing params in tool call".to_string())?;

    let tool_name = params["name"]
        .as_str()
        .ok_or_else(|| "Missing tool name".to_string())?;

    let arguments = &params["arguments"];

    debug!("Tool call: {} with args: {:?}", tool_name, arguments);

    use handlers::*;
    match tool_name {
        // Session Management
        "create_session" => handle_create_session(server, arguments).await,
        "load_session" => handle_load_session(server, arguments).await,
        "list_sessions" => handle_list_sessions(server).await,
        "search_sessions" => handle_search_sessions(server, arguments).await,
        "update_session_metadata" => handle_update_session_metadata(server, arguments).await,

        // Context Operations
        "update_conversation_context" => handle_update_context(server, arguments).await,
        "query_conversation_context" => handle_query_context(server, arguments).await,
        "bulk_update_conversation_context" => handle_bulk_update_context(server, arguments).await,
        "create_session_checkpoint" => handle_create_checkpoint(server, arguments).await,

        // Semantic Search
        "semantic_search_session" => handle_semantic_search(server, arguments).await,
        "semantic_search_global" => handle_semantic_search_global(server, arguments).await,
        "find_related_content" => handle_find_related_content(server, arguments).await,
        "vectorize_session" => handle_vectorize_session(server, arguments).await,
        "get_vectorization_stats" => handle_get_vectorization_stats(server).await,

        // Analysis & Insights
        "get_structured_summary" => handle_get_summary(server, arguments).await,
        "get_key_decisions" => handle_get_key_decisions(server, arguments).await,
        "get_key_insights" => handle_get_key_insights(server, arguments).await,
        "get_entity_importance_analysis" => handle_get_entity_importance(server, arguments).await,
        "get_entity_network_view" => handle_get_entity_network(server, arguments).await,
        "get_session_statistics" => handle_get_session_statistics(server, arguments).await,
        "get_tool_catalog" => handle_get_tool_catalog(server).await,

        // Workspace Management
        "create_workspace" => handle_create_workspace(server, arguments).await,
        "get_workspace" => handle_get_workspace(server, arguments).await,
        "list_workspaces" => handle_list_workspaces(server).await,
        "delete_workspace" => handle_delete_workspace(server, arguments).await,
        "add_session_to_workspace" => handle_add_session_to_workspace(server, arguments).await,
        "remove_session_from_workspace" => {
            handle_remove_session_from_workspace(server, arguments).await
        }

        _ => Err(format!("Unknown tool: {}", tool_name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> DaemonConfig {
        DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // Random port
            grpc_port: 0,
            data_directory: tempfile::tempdir()
                .unwrap()
                .path()
                .to_str()
                .unwrap()
                .to_string(),
            storage_backend: "rocksdb".to_string(),
            surrealdb_endpoint: None,
            surrealdb_username: None,
            surrealdb_password: None,
            surrealdb_namespace: "post_cortex".to_string(),
            surrealdb_database: "main".to_string(),
        }
    }

    #[tokio::test]
    async fn test_daemon_server_creation() {
        let server = DaemonServer::new(test_config()).await;
        assert!(server.is_ok());

        let server = server.unwrap();
        assert_eq!(server.active_connections.len(), 0);
        assert_eq!(server.total_requests.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_server_statistics() {
        let server = DaemonServer::new(test_config()).await.unwrap();

        let stats = server.get_statistics();
        assert_eq!(stats.active_connections, 0);
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.workspace_count, 0);
    }
}
