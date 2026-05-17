// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! REST API for the local `pcx` CLI.
//!
//! Lives alongside the MCP JSON-RPC transport. CLI talks to the daemon over
//! `/api/*` so we don't pay the JSON-RPC framing tax for what are essentially
//! CRUD calls.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

use super::DaemonServer;

/// Error type for HTTP handlers
#[allow(dead_code)]
#[derive(Debug)]
pub(super) struct AppError(pub(super) String);

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

/// Session info for API responses
#[derive(Serialize)]
pub(super) struct SessionInfo {
    id: String,
    name: String,
    workspace: Option<String>,
}

/// Workspace info for API responses
#[derive(Serialize)]
pub(super) struct WorkspaceInfo {
    id: String,
    name: String,
    description: String,
    session_count: usize,
}

#[derive(Deserialize)]
pub(super) struct CreateSessionRequest {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CreateWorkspaceRequest {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct AttachSessionRequest {
    role: Option<String>,
}

/// List all sessions
pub(super) async fn api_list_sessions(
    State(server): State<Arc<DaemonServer>>,
) -> Result<Json<Vec<SessionInfo>>, (StatusCode, String)> {
    let ids = server
        .memory_system
        .list_sessions()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
pub(super) async fn api_create_session(
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
pub(super) async fn api_delete_session(
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
pub(super) async fn api_list_workspaces(
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
pub(super) async fn api_create_workspace(
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
pub(super) async fn api_delete_workspace(
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
pub(super) async fn api_attach_session(
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
        "primary" => post_cortex_core::workspace::SessionRole::Primary,
        "related" => post_cortex_core::workspace::SessionRole::Related,
        "dependency" => post_cortex_core::workspace::SessionRole::Dependency,
        "shared" => post_cortex_core::workspace::SessionRole::Shared,
        other => return Err((StatusCode::BAD_REQUEST, format!("Invalid role: {}", other))),
    };

    server
        .memory_system
        .get_storage()
        .add_session_to_workspace(ws_id, sess_id, role.clone())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let _ = server
        .memory_system
        .workspace_manager
        .add_session_to_workspace(&ws_id, sess_id, role);

    Ok(StatusCode::OK)
}
