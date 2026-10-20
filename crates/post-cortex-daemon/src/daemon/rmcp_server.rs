// Copyright (c) 2025 Julius ML
// MIT License

//! RMCP-based SSE Server for Post-Cortex daemon
//!
//! Provides SSE transport using rmcp library following official shuttle patterns.

use crate::daemon::host_allowlist::{HostPolicy, require_allowed_host};
use crate::daemon::{DaemonConfig, PostCortexService};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::error::SystemError;
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::{
    AppendMessagesRequest, AppendMessagesResponse, GetCardRequest, PostCortexService as _,
    UpdateCardRequest, UpdateCardResponse,
};
use post_cortex_memory::ConversationMemorySystem;
use post_cortex_memory::services::MemoryServiceImpl;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Start RMCP-based SSE server
pub async fn start_rmcp_daemon(config: DaemonConfig) -> Result<(), String> {
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    let host_policy = HostPolicy::from_config(&config)?;

    info!("Initializing Post-Cortex daemon with RMCP SSE transport");
    info!("  Host: {}", config.host);
    info!("  Port: {}", config.port);
    info!("  Data Directory: {}", config.data_directory);
    log_host_policy(&host_policy, &addr, config.allowed_hosts.is_empty());

    // Create memory system with embeddings enabled
    let mut system_config = post_cortex_memory::SystemConfig {
        data_directory: config.data_directory.clone(),
        redaction: post_cortex_core::redact::RedactionConfig {
            enabled: config.redact,
            extra_patterns: config.redact_patterns.clone(),
        },
        ..Default::default()
    };
    info!(
        "  Redaction: {} ({} extra patterns)",
        if config.redact { "on" } else { "OFF" },
        config.redact_patterns.len()
    );

    #[cfg(feature = "embeddings")]
    {
        // Inherit `embeddings_model_type` + `vector_dimension` from
        // `SystemConfig::default()` (PotionMultilingual / 256-dim as of
        // 0.3.0) — hard-coding `"MultilingualMiniLM"` here used to
        // silently downgrade the embedding model regardless of the
        // workspace default and pinned the HNSW index to 384-dim,
        // mismatching the runtime vectoriser.
        system_config.enable_embeddings = true;
        system_config.auto_vectorize_on_update = true;
        system_config.cross_session_search_enabled = true;
        system_config.reranker_model = config.reranker_model.clone();
        system_config.neighbour_radius = config.neighbour_radius;
        system_config.voyage_api_key = config.voyage_api_key.clone();
        system_config.voyage_model = config.voyage_model.clone();
        system_config.search_mode = config.search_mode.clone();
        info!(
            "Embeddings enabled in daemon config (model={}, dim={})",
            system_config.embeddings_model_type, system_config.vector_dimension,
        );
    }

    // Configure storage backend if surrealdb-storage feature is enabled
    #[cfg(feature = "surrealdb-storage")]
    {
        use post_cortex_storage::traits::StorageBackendType;

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

    info!("Memory system initialized successfully");

    // Load the embedding model and stored vectors in the background so the
    // first search after a restart is not the one that pays for it.
    #[cfg(feature = "embeddings")]
    {
        let warm = memory_system.clone();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            match warm.warm_up_embeddings().await {
                Ok(()) => info!("Embeddings warmed up in {:?}", started.elapsed()),
                Err(e) => error!("Embedding warm-up failed (search will retry lazily): {e}"),
            }
        });
    }

    // Inject memory system into MCP tools so they use shared instance
    let service = post_cortex_mcp::inject_memory_system(memory_system.clone());
    info!("Memory system injected into MCP tools");

    // Clear query cache to prevent stale vector IDs from previous runs
    if let Err(e) = memory_system.clear_query_cache().await {
        error!("Failed to clear query cache on startup: {}", e);
    } else {
        info!("Query cache cleared successfully on daemon startup");
    }

    // Create cancellation token for graceful shutdown
    let ct = CancellationToken::new();

    spawn_message_retention(memory_system.clone(), config.message_retention_days, &ct);

    let router = build_router(
        memory_system.clone(),
        service,
        &host_policy,
        config.auth_token.clone(),
        &ct,
    );
    info!("MCP endpoint configured: http://{}/mcp", addr);
    match &config.auth_token {
        Some(_) => info!("  Auth: bearer token required (HTTP except /health, gRPC)"),
        None if !addr.ip().is_loopback() => {
            warn!(
                "  Auth: NONE on non-loopback bind {} — set auth_token / PC_AUTH_TOKEN",
                addr
            );
        }
        None => info!("  Auth: none (loopback bind)"),
    }

    // Create TCP listener
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;

    info!("TCP listener bound to {}", addr);

    // Setup graceful shutdown
    let shutdown_ct = ct.clone();
    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        shutdown_ct.cancelled().await;
        info!("HTTP server shutting down gracefully");
    });

    // Start HTTP server in background
    tokio::spawn(async move {
        if let Err(e) = server.await {
            error!("HTTP server error: {}", e);
        }
    });

    // Start gRPC server alongside HTTP if configured
    if config.grpc_port > 0 {
        let grpc_addr: SocketAddr = format!("{}:{}", config.host, config.grpc_port)
            .parse()
            .map_err(|e| format!("Invalid gRPC address: {}", e))?;
        let grpc_memory = memory_system.clone();
        let grpc_token = config.auth_token.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::daemon::grpc_service::start_grpc_server(grpc_memory, grpc_addr, grpc_token)
                    .await
            {
                error!("gRPC server error: {}", e);
            }
        });
        info!("gRPC endpoint: {}:{}", config.host, config.grpc_port);
    }

    info!("Post-Cortex MCP service registered with SSE server");
    info!("Daemon is ready to accept connections");
    info!("Press Ctrl+C to shutdown");

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, initiating shutdown");
        }
        _ = terminate_signal() => {
            info!("Received SIGTERM, initiating shutdown");
        }
        _ = ct.cancelled() => {
            info!("Service cancelled");
        }
    }

    ct.cancel();
    // Session persists run in the background; write them before exiting,
    // or the last updates of every session would be lost with the process.
    memory_system
        .flush_all(std::time::Duration::from_secs(30))
        .await;
    info!("Post-Cortex daemon stopped");
    Ok(())
}

/// How often expired messages are deleted.
const MESSAGE_RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Delete captured messages older than `retention_days` at startup and then
/// daily, until `ct` is cancelled. `0` keeps messages forever.
fn spawn_message_retention(
    memory_system: Arc<ConversationMemorySystem>,
    retention_days: u32,
    ct: &CancellationToken,
) {
    if retention_days == 0 {
        info!("  Message retention: forever");
        return;
    }
    info!("  Message retention: {} days", retention_days);
    let ct = ct.child_token();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(MESSAGE_RETENTION_INTERVAL);
        loop {
            tokio::select! {
                _ = ct.cancelled() => break,
                _ = interval.tick() => {}
            }
            let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(retention_days));
            if let Err(e) = memory_system.expire_messages(cutoff).await {
                error!("Message retention failed: {e}");
            }
        }
    });
}

/// Builds the HTTP app: rmcp's Streamable HTTP service on `/mcp`, the REST
/// API on `/api/*` and `/health`.
///
/// `host_policy` is enforced by rmcp on `/mcp` and by
/// [`require_allowed_host`] on `/api/*`; `/health` accepts any `Host`. With
/// `auth_token` set every route but `/health` also needs the bearer token.
pub fn build_router(
    memory_system: Arc<ConversationMemorySystem>,
    service: Arc<MemoryServiceImpl>,
    host_policy: &HostPolicy,
    auth_token: Option<String>,
    ct: &CancellationToken,
) -> Router {
    let mcp_service = StreamableHttpService::new(
        {
            let memory_system = memory_system.clone();
            move || Ok(PostCortexService::new(memory_system.clone()))
        },
        LocalSessionManager::default().into(),
        host_policy
            .apply(StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token())),
    );

    let api_state = Arc::new(ApiState {
        memory_system,
        service,
    });
    let api_router = Router::new()
        .route("/health", get(api_health))
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
        .route("/api/messages", post(api_append_messages))
        .route("/api/cards", get(api_get_cards))
        .route("/api/cards/user", put(api_put_user_card))
        .route("/api/cards/workspace/{id}", put(api_put_workspace_card))
        .with_state(api_state)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(host_policy.clone()),
            require_allowed_host,
        ));

    let router = Router::new()
        .nest_service("/mcp", mcp_service)
        .merge(api_router);
    match auth_token {
        Some(token) => router.layer(axum::middleware::from_fn_with_state(
            Arc::new(token),
            crate::daemon::auth::require_bearer,
        )),
        None => router,
    }
}

/// Logs the effective `Host` allowlist, warning when it will turn remote
/// clients away or is switched off.
fn log_host_policy(policy: &HostPolicy, addr: &SocketAddr, using_defaults: bool) {
    match policy {
        HostPolicy::Any => warn!(
            "  Allowed hosts: ANY (allowed_hosts = [\"*\"]) — Host validation is off, \
             the daemon is exposed to DNS rebinding from any web page"
        ),
        HostPolicy::Only(hosts) => {
            info!("  Allowed hosts: {}", hosts.join(", "));
            if addr.ip().is_unspecified() && using_defaults {
                warn!(
                    "  Bound to {} but allowed_hosts only lists loopback names: remote \
                     clients get 403 on /mcp and /api/* until allowed_hosts (or \
                     PC_ALLOWED_HOSTS) lists the host names they connect with, e.g. a \
                     tailnet hostname or IP",
                    addr
                );
            }
        }
    }
}

/// Resolves on SIGTERM (what launchd, systemd and `kill` send); never on
/// platforms without it.
async fn terminate_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                term.recv().await;
            }
            Err(e) => {
                warn!("Cannot listen for SIGTERM: {}", e);
                std::future::pending::<()>().await;
            }
        }
    }
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}

// ============================================================================
// REST API for CLI
// ============================================================================

/// Shared state for API handlers
struct ApiState {
    memory_system: Arc<ConversationMemorySystem>,
    /// Canonical service (redaction, validation) for the write endpoints.
    service: Arc<MemoryServiceImpl>,
}

#[derive(Serialize)]
struct SessionInfo {
    id: String,
    name: String,
    workspace: Option<String>,
}

#[derive(Serialize)]
struct WorkspaceInfo {
    id: String,
    name: String,
    description: String,
    session_count: usize,
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct CreateWorkspaceRequest {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct AttachSessionRequest {
    role: Option<String>,
}

/// Body of `POST /api/messages`: captured turns from a client hook.
#[derive(Deserialize)]
struct AppendMessagesBody {
    session_id: Uuid,
    messages: Vec<MessageBody>,
}

/// One captured turn; see [`NewMessage`] for the fields.
#[derive(Deserialize)]
struct MessageBody {
    turn_id: String,
    role: MessageRole,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    source_session: Option<String>,
    /// Defaults to the time of the request.
    #[serde(default)]
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    text: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
}

impl MessageBody {
    fn into_new_message(self, now: chrono::DateTime<chrono::Utc>) -> NewMessage {
        NewMessage {
            turn_id: self.turn_id,
            role: self.role,
            author: self.author,
            source_session: self.source_session,
            timestamp: self.timestamp.unwrap_or(now),
            text: self.text,
            tools: self.tools,
            files: self.files,
        }
    }
}

async fn api_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "post-cortex"
    }))
}

/// Query of `GET /api/cards`.
#[derive(Deserialize)]
struct CardsQuery {
    /// The project whose card to return with the user card.
    workspace_id: Option<Uuid>,
}

/// Body of `GET /api/cards`: the cards a session starts with.
#[derive(Serialize)]
struct CardsBody {
    user: Option<Card>,
    workspace: Option<Card>,
}

/// Body of `PUT /api/cards/...`.
#[derive(Deserialize)]
struct PutCardBody {
    content: String,
    /// Version the content was edited from (0: no card yet).
    expected_version: u64,
    #[serde(default)]
    author: Option<String>,
}

/// The user card and, with `workspace_id`, the project card (ROADMAP
/// Phase 2 item 3); read by the SessionStart hook and `pcx card`.
async fn api_get_cards(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<CardsQuery>,
) -> Result<Json<CardsBody>, (StatusCode, String)> {
    let card = |scope| {
        let service = Arc::clone(&state.service);
        async move {
            service
                .get_card(GetCardRequest { scope, history: 0 })
                .await
                .map(|r| r.card)
                .map_err(card_error)
        }
    };
    let user = card(CardScope::User).await?;
    let workspace = match query.workspace_id {
        Some(id) => card(CardScope::Workspace(id)).await?,
        None => None,
    };
    Ok(Json(CardsBody { user, workspace }))
}

async fn api_put_user_card(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<PutCardBody>,
) -> Result<(StatusCode, Json<UpdateCardResponse>), (StatusCode, String)> {
    put_card(&state, CardScope::User, body).await
}

async fn api_put_workspace_card(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<PutCardBody>,
) -> Result<(StatusCode, Json<UpdateCardResponse>), (StatusCode, String)> {
    put_card(&state, CardScope::Workspace(id), body).await
}

/// 200 with the new version, or 409 with the latest one when the card
/// changed since `expected_version`.
async fn put_card(
    state: &ApiState,
    scope: CardScope,
    body: PutCardBody,
) -> Result<(StatusCode, Json<UpdateCardResponse>), (StatusCode, String)> {
    let response = state
        .service
        .update_card(UpdateCardRequest {
            scope,
            text: body.content,
            expected_version: body.expected_version,
            author: body.author,
        })
        .await
        .map_err(card_error)?;
    let status = if response.written {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    Ok((status, Json(response)))
}

fn card_error(e: SystemError) -> (StatusCode, String) {
    match e {
        SystemError::WorkspaceNotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
        SystemError::InvalidArgument(_) => (StatusCode::BAD_REQUEST, e.to_string()),
        e => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Append captured turns to a session's message stream (ROADMAP Phase 2
/// item 1). Idempotent on each turn's `turn_id`, so hooks can retry.
async fn api_append_messages(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<AppendMessagesBody>,
) -> Result<Json<AppendMessagesResponse>, (StatusCode, String)> {
    let now = chrono::Utc::now();
    let request = AppendMessagesRequest {
        session_id: body.session_id,
        messages: body
            .messages
            .into_iter()
            .map(|m| m.into_new_message(now))
            .collect(),
    };
    match state.service.append_messages(request).await {
        Ok(response) => Ok(Json(response)),
        Err(e @ SystemError::SessionNotFound(_)) => Err((StatusCode::NOT_FOUND, e.to_string())),
        Err(e @ SystemError::InvalidArgument(_)) => Err((StatusCode::BAD_REQUEST, e.to_string())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

async fn api_list_sessions(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<SessionInfo>>, (StatusCode, String)> {
    let ids = state
        .memory_system
        .list_session_summaries()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let workspaces = state.memory_system.workspace_manager.list_workspaces();
    let mut session_workspace_map = std::collections::HashMap::new();
    for ws in workspaces {
        for (session_id, _role) in ws.get_all_sessions() {
            session_workspace_map.insert(session_id, ws.name.clone());
        }
    }

    // Metadata records only: listing never loads sessions.
    let sessions = ids
        .into_iter()
        .map(|summary| SessionInfo {
            id: summary.id.to_string(),
            name: summary.name.unwrap_or_else(|| "Unnamed".to_string()),
            workspace: session_workspace_map.get(&summary.id).cloned(),
        })
        .collect();

    Ok(Json(sessions))
}

async fn api_create_session(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionInfo>, (StatusCode, String)> {
    let id = state
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

async fn api_delete_session(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid UUID: {}", e)))?;

    state
        .memory_system
        .delete_session(uuid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn api_list_workspaces(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<WorkspaceInfo>>, (StatusCode, String)> {
    let workspaces = state
        .memory_system
        .get_storage()
        .list_all_workspaces()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Get list of existing sessions to filter out deleted ones
    let existing_sessions: std::collections::HashSet<_> = state
        .memory_system
        .list_sessions()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .into_iter()
        .collect();

    let result: Vec<WorkspaceInfo> = workspaces
        .into_iter()
        .map(|ws| {
            // Count only sessions that still exist
            let actual_count = ws
                .sessions
                .iter()
                .filter(|(id, _)| existing_sessions.contains(id))
                .count();
            WorkspaceInfo {
                id: ws.id.to_string(),
                name: ws.name,
                description: ws.description,
                session_count: actual_count,
            }
        })
        .collect();

    Ok(Json(result))
}

async fn api_create_workspace(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<Json<WorkspaceInfo>, (StatusCode, String)> {
    let id = Uuid::new_v4();
    let description = req.description.unwrap_or_default();

    state
        .memory_system
        .get_storage()
        .save_workspace_metadata(id, &req.name, &description, &[])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Update in-memory workspace manager
    state.memory_system.workspace_manager.restore_workspace(
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

async fn api_delete_workspace(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid UUID: {}", e)))?;

    state
        .memory_system
        .get_storage()
        .delete_workspace(uuid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Update in-memory workspace manager
    state
        .memory_system
        .workspace_manager
        .delete_workspace(&uuid);

    Ok(StatusCode::NO_CONTENT)
}

async fn api_attach_session(
    State(state): State<Arc<ApiState>>,
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
        "primary" => post_cortex_core::workspace::SessionRole::Primary,
        "related" => post_cortex_core::workspace::SessionRole::Related,
        "dependency" => post_cortex_core::workspace::SessionRole::Dependency,
        "shared" => post_cortex_core::workspace::SessionRole::Shared,
        other => return Err((StatusCode::BAD_REQUEST, format!("Invalid role: {}", other))),
    };

    state
        .memory_system
        .get_storage()
        .add_session_to_workspace(ws_id, sess_id, role)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Update in-memory workspace manager to keep it in sync
    let _ = state
        .memory_system
        .workspace_manager
        .add_session_to_workspace(&ws_id, sess_id, role);

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use tower::ServiceExt;

    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"host-test","version":"0"}}}"#;

    async fn memory_system(dir: &tempfile::TempDir) -> Arc<ConversationMemorySystem> {
        let config = post_cortex_memory::SystemConfig {
            data_directory: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        };
        Arc::new(ConversationMemorySystem::new(config).await.unwrap())
    }

    fn policy(host: &str, allowed: &[&str]) -> HostPolicy {
        HostPolicy::from_config(&DaemonConfig {
            host: host.to_string(),
            allowed_hosts: allowed.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        })
        .unwrap()
    }

    fn initialize(host: &str) -> Request<Body> {
        Request::post("/mcp")
            .header(header::HOST, host)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(Body::from(INITIALIZE))
            .unwrap()
    }

    fn get(path: &str, host: &str) -> Request<Body> {
        Request::get(path)
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap()
    }

    async fn status_and_body(router: &Router, req: Request<Body>) -> (StatusCode, String) {
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        // Initialize answers on an SSE stream that stays open; the first
        // frame holds the result.
        let body = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            to_bytes(resp.into_body(), 64 * 1024),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
        (status, body)
    }

    #[tokio::test]
    async fn host_allowlist_guards_mcp_and_api() {
        let dir = tempfile::tempdir().unwrap();
        let memory = memory_system(&dir).await;
        let ct = CancellationToken::new();
        let router = build_router(
            memory.clone(),
            Arc::new(MemoryServiceImpl::new(memory)),
            &policy("0.0.0.0", &["pc.tailnet.example", "100.64.0.7:3737"]),
            None,
            &ct,
        );

        // DNS rebinding: foreign Host gets 403 on /mcp and /api/*.
        for host in ["evil.example", "evil.example:3737", "100.64.0.7:80"] {
            let (status, body) = status_and_body(&router, initialize(host)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "/mcp with Host {host}");
            assert!(body.contains("Host header is not allowed"), "{body}");
            let (status, _) = status_and_body(&router, get("/api/sessions", host)).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "/api/sessions with Host {host}"
            );
        }

        // Loopback and configured hosts are served.
        for host in [
            "localhost:3737",
            "127.0.0.1:3737",
            "[::1]:3737",
            "pc.tailnet.example:3737",
            "100.64.0.7:3737",
        ] {
            let (status, body) = status_and_body(&router, initialize(host)).await;
            assert_eq!(status, StatusCode::OK, "/mcp with Host {host}");
            assert!(
                body.contains("\"protocolVersion\":\"2024-11-05\""),
                "{body}"
            );
            let (status, _) = status_and_body(&router, get("/api/sessions", host)).await;
            assert_eq!(status, StatusCode::OK, "/api/sessions with Host {host}");
        }

        // Liveness probe stays open.
        let (status, _) = status_and_body(&router, get("/health", "evil.example")).await;
        assert_eq!(status, StatusCode::OK);
        ct.cancel();
    }

    fn post_json(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::post(path)
            .header(header::HOST, "localhost:3737")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// `POST /api/messages` stores turns through the canonical service:
    /// unknown sessions are 404, secrets are redacted, retries are skipped.
    #[tokio::test]
    async fn api_messages_append_redacted_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Arc::new(
            ConversationMemorySystem::new(post_cortex_memory::SystemConfig {
                data_directory: dir.path().to_str().unwrap().to_string(),
                enable_embeddings: false,
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        let ct = CancellationToken::new();
        let router = build_router(
            memory.clone(),
            Arc::new(MemoryServiceImpl::new(memory.clone())),
            &policy("127.0.0.1", &[]),
            None,
            &ct,
        );
        let body = |session: Uuid| {
            serde_json::json!({
                "session_id": session,
                "messages": [
                    {"turn_id": "u1", "role": "user", "text": "deploy with key AKIAABCDEFGHIJKLMNOP"},
                    {"turn_id": "a1", "role": "assistant", "text": "done", "author": "claude",
                     "tools": ["Bash"], "files": ["deploy.sh"]},
                    {"turn_id": "a2", "role": "assistant", "text": "   "}
                ]
            })
        };

        let (status, _) =
            status_and_body(&router, post_json("/api/messages", body(Uuid::new_v4()))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let sid = memory.create_session(None, None).await.unwrap();
        let (status, text) = status_and_body(&router, post_json("/api/messages", body(sid))).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let response: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(response["appended"], serde_json::json!([0, 1]));
        assert_eq!(response["skipped"], 1, "the empty turn");

        let (status, text) = status_and_body(&router, post_json("/api/messages", body(sid))).await;
        assert_eq!(status, StatusCode::OK);
        let response: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(response["appended"], serde_json::json!([]));
        assert_eq!(response["skipped"], 3);

        let stored = memory
            .storage_actor
            .load_messages(sid, 0..10)
            .await
            .unwrap();
        assert_eq!(stored.len(), 2);
        assert!(!stored[0].text.contains("AKIA"), "{}", stored[0].text);
        assert!(stored[0].text.contains("[REDACTED:"), "{}", stored[0].text);
        assert_eq!(stored[1].author.as_deref(), Some("claude"));
        assert_eq!(stored[1].tools, ["Bash"]);

        let (status, _) = status_and_body(
            &router,
            post_json(
                "/api/messages",
                serde_json::json!({"session_id": sid, "messages": [{"turn_id": "x", "text": "no role"}]}),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{status}");
        ct.cancel();
    }

    fn put_json(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::put(path)
            .header(header::HOST, "localhost:3737")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// Cards over REST: versions, 409 with the latest on a stale write, 404
    /// for an unknown workspace, and GET returning both cards.
    #[tokio::test]
    async fn api_cards_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Arc::new(
            ConversationMemorySystem::new(post_cortex_memory::SystemConfig {
                data_directory: dir.path().to_str().unwrap().to_string(),
                enable_embeddings: false,
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        let ct = CancellationToken::new();
        let router = build_router(
            memory.clone(),
            Arc::new(MemoryServiceImpl::new(memory.clone())),
            &policy("127.0.0.1", &[]),
            None,
            &ct,
        );
        let ws = memory
            .workspace_manager
            .create_workspace("cards".to_string(), String::new());
        let put = |path: String, content: &str, expected_version: u64| {
            put_json(
                &path,
                serde_json::json!({"content": content, "expected_version": expected_version}),
            )
        };
        let json = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();

        let (status, _) = status_and_body(
            &router,
            put(format!("/api/cards/workspace/{}", Uuid::new_v4()), "x", 0),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let path = format!("/api/cards/workspace/{ws}");
        let (status, text) = status_and_body(&router, put(path.clone(), "## Stack\nrust", 0)).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(json(&text)["card"]["version"], 1);

        let (status, text) = status_and_body(&router, put(path.clone(), "stale", 0)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json(&text)["card"]["text"], "## Stack\nrust");

        let (status, _) =
            status_and_body(&router, put("/api/cards/user".to_string(), "be brief", 0)).await;
        assert_eq!(status, StatusCode::OK);

        let (status, text) = status_and_body(
            &router,
            get(&format!("/api/cards?workspace_id={ws}"), "localhost:3737"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let cards = json(&text);
        assert_eq!(cards["user"]["text"], "be brief");
        assert_eq!(cards["workspace"]["version"], 1);
        assert_eq!(cards["workspace"]["scope"]["workspace"], ws.to_string());

        let (_, text) = status_and_body(&router, get("/api/cards", "localhost:3737")).await;
        assert!(json(&text)["workspace"].is_null());
        ct.cancel();
    }

    #[tokio::test]
    async fn star_disables_host_validation() {
        let dir = tempfile::tempdir().unwrap();
        let ct = CancellationToken::new();
        let memory = memory_system(&dir).await;
        let router = build_router(
            memory.clone(),
            Arc::new(MemoryServiceImpl::new(memory)),
            &policy("0.0.0.0", &["*"]),
            None,
            &ct,
        );
        let (status, _) = status_and_body(&router, initialize("evil.example")).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = status_and_body(&router, get("/api/sessions", "evil.example")).await;
        assert_eq!(status, StatusCode::OK);
        ct.cancel();
    }
}
