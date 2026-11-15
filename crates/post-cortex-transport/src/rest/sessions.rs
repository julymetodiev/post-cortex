// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! REST routes for sessions and workspaces: list, create, delete, attach.

use crate::rest::ApiState;
use crate::rest::hooks::Params;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use post_cortex_memory::capabilities::CreateWorkspace;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Serialize)]
pub struct SessionInfo {
    id: String,
    name: String,
    workspace: Option<String>,
}

#[derive(Serialize)]
pub struct WorkspaceInfo {
    id: String,
    name: String,
    slug: String,
    description: String,
    session_count: usize,
    /// Present (true) only for archived workspaces, listed with
    /// `?include_archived=true`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    archived: bool,
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateWorkspaceRequest {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
pub struct AttachSessionRequest {
    role: Option<String>,
}

pub async fn api_list_sessions(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<SessionInfo>>, (StatusCode, String)> {
    let ids = state
        .memory
        .sessions
        .list_summaries()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let workspaces = state.memory.workspaces.list(true);
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

pub async fn api_create_session(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionInfo>, (StatusCode, String)> {
    let id = state
        .memory
        .sessions
        .create(req.name.clone(), req.description)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(SessionInfo {
        id: id.to_string(),
        name: req.name.unwrap_or_else(|| "Unnamed".to_string()),
        workspace: None,
    }))
}

pub async fn api_delete_session(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid UUID: {}", e)))?;

    state
        .memory
        .sessions
        .delete(uuid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn api_list_workspaces(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<Params>,
) -> Result<Json<Vec<WorkspaceInfo>>, (StatusCode, String)> {
    let include_archived = params
        .get("include_archived")
        .is_some_and(|v| matches!(v.as_str(), "true" | "1" | "yes"));
    let mut workspaces = state.memory.workspaces.list(include_archived);
    workspaces.sort_by_key(|ws| ws.id);

    // Get list of existing sessions to filter out deleted ones
    let existing_sessions: std::collections::HashSet<_> = state
        .memory
        .sessions
        .list()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .into_iter()
        .collect();

    let result: Vec<WorkspaceInfo> = workspaces
        .into_iter()
        .map(|ws| {
            // Count only sessions that still exist
            let actual_count = ws
                .get_all_sessions()
                .iter()
                .filter(|(id, _)| existing_sessions.contains(id))
                .count();
            WorkspaceInfo {
                id: ws.id.to_string(),
                name: ws.name.clone(),
                slug: ws.slug.clone(),
                description: ws.description.clone(),
                session_count: actual_count,
                archived: ws.is_archived(),
            }
        })
        .collect();

    Ok(Json(result))
}

pub async fn api_create_workspace(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<Json<WorkspaceInfo>, (StatusCode, String)> {
    let workspace = state
        .memory
        .workspaces
        .create(CreateWorkspace {
            name: req.name,
            description: req.description.unwrap_or_default(),
        })
        .await
        .map_err(crate::rest::rest_status)?;

    Ok(Json(WorkspaceInfo {
        id: workspace.id.to_string(),
        name: workspace.name.clone(),
        slug: workspace.slug.clone(),
        description: workspace.description.clone(),
        session_count: 0,
        archived: false,
    }))
}

pub async fn api_delete_workspace(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid UUID: {}", e)))?;

    // The one locked delete: storage first, then memory.
    state
        .memory
        .workspaces
        .delete(uuid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn api_attach_session(
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

    let role: post_cortex_core::workspace::SessionRole =
        post_cortex_memory::tools::wire::parse("role", req.role.as_deref().unwrap_or("related"))
            .map_err(crate::rest::rest_status)?;

    state
        .memory
        .workspaces
        .add_session(ws_id, sess_id, role)
        .await
        .map_err(crate::rest::rest_status)?;

    Ok(StatusCode::OK)
}
