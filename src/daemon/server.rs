// Copyright (c) 2025 Julius ML
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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Lock-free HTTP daemon server
//!
//! Provides HTTP/JSON-RPC endpoint for MCP protocol with zero blocking operations.

use crate::ConversationMemorySystem;
use crate::daemon::sse::SSEBroadcaster;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{delete, get, post},
};
use dashmap::DashMap;
use futures::stream::{self};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info};
use uuid::Uuid;

// DaemonConfig is now in config.rs module
use super::config::DaemonConfig;

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
    memory_system: Arc<ConversationMemorySystem>,

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

    /// Build Axum router for testing without starting TCP server
    ///
    /// This method exposes the router for in-memory HTTP testing,
    /// eliminating the need for real TCP ports in tests.
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
                get(api_list_sessions).post(api_create_session),
            )
            .route("/api/sessions/{id}", delete(api_delete_session))
            .route(
                "/api/workspaces",
                get(api_list_workspaces).post(api_create_workspace),
            )
            .route("/api/workspaces/{id}", delete(api_delete_workspace))
            .route(
                "/api/workspaces/{workspace_id}/sessions/{session_id}",
                post(api_attach_session),
            )
            .layer(CorsLayer::permissive())
            .with_state(server)
    }

    /// Start HTTP server
    pub async fn start(self) -> Result<(), String> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| format!("Invalid address: {}", e))?;

        // Build router
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

/// MCP JSON-RPC request
#[derive(Debug, Deserialize)]
struct MCPRequest {
    #[allow(dead_code)] // Used for validation
    jsonrpc: String,
    id: serde_json::Value,
    method: String,
    #[allow(dead_code)] // Will be used for tool calls
    params: Option<serde_json::Value>,
}

/// MCP JSON-RPC response
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

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "post-cortex-daemon"
    }))
}

/// Statistics endpoint
async fn get_stats(State(server): State<Arc<DaemonServer>>) -> impl IntoResponse {
    Json(server.get_statistics())
}

/// SSE stream endpoint for Streamable HTTP transport
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

/// MCP request handler - Streamable HTTP transport (2025-03-26)
/// Returns response directly as JSON (not via SSE)
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

async fn handle_create_session(
    server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let name = arguments["name"].as_str().map(|s| s.to_string());
    let description = arguments["description"].as_str().map(|s| s.to_string());

    let session_id = server
        .memory_system
        .create_session(name, description)
        .await
        .map_err(|e| format!("Failed to create session: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": format!("Created new session: {}", session_id)
        }]
    }))
}

async fn handle_load_session(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::load_session;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;

    let uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| format!("Invalid session_id: {}", e))?;

    let result = load_session(uuid)
        .await
        .map_err(|e| format!("Failed to load session: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_update_context(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::core::context_update::CodeReference;
    use crate::tools::mcp::update_conversation_context;

    let interaction_type = arguments["interaction_type"]
        .as_str()
        .ok_or_else(|| "Missing interaction_type".to_string())?
        .to_string();

    // Convert content from JSON to HashMap
    let content_obj = arguments["content"]
        .as_object()
        .ok_or_else(|| "Content must be an object".to_string())?;

    let mut content = HashMap::new();
    for (key, value) in content_obj {
        let value_str = value
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| value.to_string());
        content.insert(key.clone(), value_str);
    }

    // Parse code_reference if provided
    let code_reference: Option<CodeReference> = arguments
        .get("code_reference")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    // Parse session_id
    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let result = update_conversation_context(interaction_type, content, code_reference, session_id)
        .await
        .map_err(|e| format!("Failed to update context: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_semantic_search(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::semantic_search_session;

    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let query = arguments["query"]
        .as_str()
        .ok_or_else(|| "Missing query".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let date_from = arguments
        .get("date_from")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let date_to = arguments
        .get("date_to")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let interaction_type = arguments
        .get("interaction_type")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        });

    let recency_bias = arguments
        .get("recency_bias")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let result = semantic_search_session(
        session_id,
        query,
        limit,
        date_from,
        date_to,
        interaction_type,
        recency_bias,
    )
    .await
    .map_err(|e| format!("Failed to search: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_list_sessions(_server: &Arc<DaemonServer>) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::list_sessions;

    let result = list_sessions()
        .await
        .map_err(|e| format!("Failed to list sessions: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_query_context(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::query_conversation_context;

    let query_type = arguments["query_type"]
        .as_str()
        .ok_or_else(|| "Missing query_type".to_string())?
        .to_string();

    // Convert parameters from JSON to HashMap
    let params_obj = arguments["parameters"]
        .as_object()
        .ok_or_else(|| "Parameters must be an object".to_string())?;

    let mut parameters = HashMap::new();
    for (key, value) in params_obj {
        let value_str = value
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| value.to_string());
        parameters.insert(key.clone(), value_str);
    }

    // Parse session_id
    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let result = query_conversation_context(query_type, parameters, session_id)
        .await
        .map_err(|e| format!("Failed to query context: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_bulk_update_context(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::{ContextUpdateItem, bulk_update_conversation_context};

    // Parse session_id
    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    // Parse updates array into Vec<ContextUpdateItem>
    let updates_json = arguments["updates"]
        .as_array()
        .ok_or_else(|| "Updates must be an array".to_string())?;

    let mut updates: Vec<ContextUpdateItem> = Vec::new();
    for update_val in updates_json {
        let update: ContextUpdateItem = serde_json::from_value(update_val.clone())
            .map_err(|e| format!("Failed to parse update: {}", e))?;
        updates.push(update);
    }

    let result = bulk_update_conversation_context(updates, session_id)
        .await
        .map_err(|e| format!("Failed to bulk update: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_semantic_search_global(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::semantic_search_global;

    let query = arguments["query"]
        .as_str()
        .ok_or_else(|| "Missing query".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let date_from = arguments
        .get("date_from")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let date_to = arguments
        .get("date_to")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let interaction_type = arguments
        .get("interaction_type")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        });

    let recency_bias = arguments
        .get("recency_bias")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let result = semantic_search_global(
        query,
        limit,
        date_from,
        date_to,
        interaction_type,
        recency_bias,
    )
    .await
    .map_err(|e| format!("Failed to search globally: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_find_related_content(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::find_related_content;

    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let topic = arguments["topic"]
        .as_str()
        .ok_or_else(|| "Missing topic".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let result = find_related_content(session_id, topic, limit)
        .await
        .map_err(|e| format!("Failed to find related content: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_search_sessions(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::search_sessions;

    let query = arguments["query"]
        .as_str()
        .ok_or_else(|| "Missing query".to_string())?
        .to_string();

    let result = search_sessions(query)
        .await
        .map_err(|e| format!("Failed to search sessions: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_update_session_metadata(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::update_session_metadata;

    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let name = arguments
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = arguments
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let result = update_session_metadata(session_id, name, description)
        .await
        .map_err(|e| format!("Failed to update metadata: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_create_checkpoint(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::create_session_checkpoint;

    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let result = create_session_checkpoint(session_id)
        .await
        .map_err(|e| format!("Failed to create checkpoint: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_key_decisions(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_key_decisions;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let result = get_key_decisions(session_id)
        .await
        .map_err(|e| format!("Failed to get key decisions: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_key_insights(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_key_insights;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let result = get_key_insights(session_id, limit)
        .await
        .map_err(|e| format!("Failed to get key insights: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_entity_importance(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_entity_importance_analysis;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let min_importance = arguments
        .get("min_importance")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let result = get_entity_importance_analysis(session_id, limit, min_importance)
        .await
        .map_err(|e| format!("Failed to get entity importance: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_entity_network(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_entity_network_view;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let center_entity = arguments
        .get("center_entity")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let max_entities = arguments
        .get("max_entities")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let max_relationships = arguments
        .get("max_relationships")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let result =
        get_entity_network_view(session_id, center_entity, max_entities, max_relationships)
            .await
            .map_err(|e| format!("Failed to get entity network: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_session_statistics(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_session_statistics;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let result = get_session_statistics(session_id)
        .await
        .map_err(|e| format!("Failed to get statistics: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_vectorize_session(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::vectorize_session;

    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let result = vectorize_session(session_id)
        .await
        .map_err(|e| format!("Failed to vectorize session: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_vectorization_stats(
    _server: &Arc<DaemonServer>,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_vectorization_stats;

    let result = get_vectorization_stats()
        .await
        .map_err(|e| format!("Failed to get vectorization stats: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_tool_catalog(_server: &Arc<DaemonServer>) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_tool_catalog;

    let result = get_tool_catalog()
        .await
        .map_err(|e| format!("Failed to get tool catalog: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_summary(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_structured_summary;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let compact = arguments.get("compact").and_then(|v| v.as_bool());
    let decisions_limit = arguments
        .get("decisions_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let entities_limit = arguments
        .get("entities_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let questions_limit = arguments
        .get("questions_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let concepts_limit = arguments
        .get("concepts_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let min_confidence = arguments
        .get("min_confidence")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let result = get_structured_summary(
        session_id,
        decisions_limit,
        entities_limit,
        questions_limit,
        concepts_limit,
        min_confidence,
        compact,
    )
    .await
    .map_err(|e| format!("Failed to get summary: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

// ============================================================================
// Workspace Management Handlers
// ============================================================================

async fn handle_create_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::create_workspace;

    let name = arguments["name"]
        .as_str()
        .ok_or_else(|| "Missing name".to_string())?
        .to_string();

    let description = arguments["description"]
        .as_str()
        .ok_or_else(|| "Missing description".to_string())?
        .to_string();

    let result = create_workspace(name, description)
        .await
        .map_err(|e| format!("Failed to create workspace: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_get_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::get_workspace;

    let workspace_id_str = arguments["workspace_id"]
        .as_str()
        .ok_or_else(|| "Missing workspace_id".to_string())?;
    let workspace_id = uuid::Uuid::parse_str(workspace_id_str)
        .map_err(|e| format!("Invalid workspace_id: {}", e))?;

    let result = get_workspace(workspace_id)
        .await
        .map_err(|e| format!("Failed to get workspace: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_list_workspaces(_server: &Arc<DaemonServer>) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::list_workspaces;

    let result = list_workspaces()
        .await
        .map_err(|e| format!("Failed to list workspaces: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_delete_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::delete_workspace;

    let workspace_id_str = arguments["workspace_id"]
        .as_str()
        .ok_or_else(|| "Missing workspace_id".to_string())?;
    let workspace_id = uuid::Uuid::parse_str(workspace_id_str)
        .map_err(|e| format!("Invalid workspace_id: {}", e))?;

    let result = delete_workspace(workspace_id)
        .await
        .map_err(|e| format!("Failed to delete workspace: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_add_session_to_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::add_session_to_workspace;

    let workspace_id_str = arguments["workspace_id"]
        .as_str()
        .ok_or_else(|| "Missing workspace_id".to_string())?;
    let workspace_id = uuid::Uuid::parse_str(workspace_id_str)
        .map_err(|e| format!("Invalid workspace_id: {}", e))?;

    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let role = arguments["role"]
        .as_str()
        .ok_or_else(|| "Missing role".to_string())?
        .to_string();

    let result = add_session_to_workspace(workspace_id, session_id, role)
        .await
        .map_err(|e| format!("Failed to add session to workspace: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

async fn handle_remove_session_from_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use crate::tools::mcp::remove_session_from_workspace;

    let workspace_id_str = arguments["workspace_id"]
        .as_str()
        .ok_or_else(|| "Missing workspace_id".to_string())?;
    let workspace_id = uuid::Uuid::parse_str(workspace_id_str)
        .map_err(|e| format!("Invalid workspace_id: {}", e))?;

    let session_id_str = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;
    let session_id =
        uuid::Uuid::parse_str(session_id_str).map_err(|e| format!("Invalid session_id: {}", e))?;

    let result = remove_session_from_workspace(workspace_id, session_id)
        .await
        .map_err(|e| format!("Failed to remove session from workspace: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": result.message
        }]
    }))
}

/// Error type for HTTP handlers
#[allow(dead_code)]
#[derive(Debug)]
struct AppError(String);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        error!("HTTP error: {}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, self.0).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: std::error::Error,
{
    fn from(err: E) -> Self {
        AppError(err.to_string())
    }
}

// ============================================================================
// REST API Handlers for CLI
// ============================================================================

/// Session info for API responses
#[derive(Serialize)]
struct SessionInfo {
    id: String,
    name: String,
    workspace: Option<String>,
}

/// Workspace info for API responses
#[derive(Serialize)]
struct WorkspaceInfo {
    id: String,
    name: String,
    description: String,
    session_count: usize,
}

/// Create session request
#[derive(Deserialize)]
struct CreateSessionRequest {
    name: Option<String>,
    description: Option<String>,
}

/// Create workspace request
#[derive(Deserialize)]
struct CreateWorkspaceRequest {
    name: String,
    description: Option<String>,
}

/// Attach session request
#[derive(Deserialize)]
struct AttachSessionRequest {
    role: Option<String>,
}

/// List all sessions
async fn api_list_sessions(
    State(server): State<Arc<DaemonServer>>,
) -> Result<Json<Vec<SessionInfo>>, (StatusCode, String)> {
    let ids = server
        .memory_system
        .list_sessions()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Build workspace map
    let workspaces = server.memory_system.workspace_manager.list_workspaces();
    let mut session_workspace_map = std::collections::HashMap::new();
    for ws in workspaces {
        for (session_id, _role) in ws.get_all_sessions() {
            session_workspace_map.insert(session_id, ws.name.clone());
        }
    }

    let mut sessions = Vec::new();
    for id in ids {
        let name = match server.memory_system.get_session(id).await {
            Ok(session_arc) => {
                let session = session_arc.load();
                session.name().unwrap_or_else(|| "Unnamed".to_string())
            }
            Err(_) => "Error loading".to_string(),
        };

        sessions.push(SessionInfo {
            id: id.to_string(),
            name,
            workspace: session_workspace_map.get(&id).cloned(),
        });
    }

    Ok(Json(sessions))
}

/// Create a new session
async fn api_create_session(
    State(server): State<Arc<DaemonServer>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionInfo>, (StatusCode, String)> {
    let id = server
        .memory_system
        .create_session(req.name.clone(), req.description)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(SessionInfo {
        id: id.to_string(),
        name: req.name.unwrap_or_else(|| "Unnamed".to_string()),
        workspace: None,
    }))
}

/// Delete a session
async fn api_delete_session(
    State(server): State<Arc<DaemonServer>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid UUID: {}", e)))?;

    server
        .memory_system
        .get_storage()
        .delete_session(uuid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::NO_CONTENT)
}

/// List all workspaces
async fn api_list_workspaces(
    State(server): State<Arc<DaemonServer>>,
) -> Result<Json<Vec<WorkspaceInfo>>, (StatusCode, String)> {
    let workspaces = server
        .memory_system
        .get_storage()
        .list_all_workspaces()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let result: Vec<WorkspaceInfo> = workspaces
        .into_iter()
        .map(|ws| WorkspaceInfo {
            id: ws.id.to_string(),
            name: ws.name,
            description: ws.description,
            session_count: ws.sessions.len(),
        })
        .collect();

    Ok(Json(result))
}

/// Create a new workspace
async fn api_create_workspace(
    State(server): State<Arc<DaemonServer>>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<Json<WorkspaceInfo>, (StatusCode, String)> {
    let id = Uuid::new_v4();
    let description = req.description.unwrap_or_default();

    server
        .memory_system
        .get_storage()
        .save_workspace_metadata(id, &req.name, &description, &[])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Update in-memory workspace manager
    server.memory_system.workspace_manager.restore_workspace(
        id,
        req.name.clone(),
        description.clone(),
        vec![],
    );

    Ok(Json(WorkspaceInfo {
        id: id.to_string(),
        name: req.name,
        description,
        session_count: 0,
    }))
}

/// Delete a workspace
async fn api_delete_workspace(
    State(server): State<Arc<DaemonServer>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid UUID: {}", e)))?;

    server
        .memory_system
        .get_storage()
        .delete_workspace(uuid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Attach session to workspace
async fn api_attach_session(
    State(server): State<Arc<DaemonServer>>,
    Path((workspace_id, session_id)): Path<(String, String)>,
    Json(req): Json<AttachSessionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let ws_id = Uuid::parse_str(&workspace_id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid workspace UUID: {}", e),
        )
    })?;
    let sess_id = Uuid::parse_str(&session_id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid session UUID: {}", e),
        )
    })?;

    let role = match req.role.as_deref().unwrap_or("related") {
        "primary" => crate::workspace::SessionRole::Primary,
        "related" => crate::workspace::SessionRole::Related,
        "dependency" => crate::workspace::SessionRole::Dependency,
        "shared" => crate::workspace::SessionRole::Shared,
        other => return Err((StatusCode::BAD_REQUEST, format!("Invalid role: {}", other))),
    };

    server
        .memory_system
        .get_storage()
        .add_session_to_workspace(ws_id, sess_id, role.clone())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Update in-memory workspace manager
    let _ = server
        .memory_system
        .workspace_manager
        .add_session_to_workspace(&ws_id, sess_id, role);

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_daemon_server_creation() {
        let config = DaemonConfig {
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
        };

        let server = DaemonServer::new(config).await;
        assert!(server.is_ok());

        let server = server.unwrap();
        assert_eq!(server.active_connections.len(), 0);
        assert_eq!(server.total_requests.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_server_statistics() {
        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
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
        };

        let server = DaemonServer::new(config).await.unwrap();

        let stats = server.get_statistics();
        assert_eq!(stats.active_connections, 0);
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.workspace_count, 0);
    }
}
