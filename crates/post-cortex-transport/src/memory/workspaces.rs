// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC workspace methods: create, get, list, delete, membership.

use crate::grpc::status;
use crate::memory::helpers::{parse_session_role, parse_uuid, workspace_to_info};
use post_cortex_memory::capabilities::CreateWorkspace;
use tonic::{Request, Response, Status};
use tracing::{debug, warn};

use post_cortex_memory::Memory;
use post_cortex_proto::pb::*;

pub async fn create_workspace(
    memory: &Memory,
    request: Request<CreateWorkspaceRequest>,
) -> Result<Response<CreateWorkspaceResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC CreateWorkspace: name={}", req.name);

    // Persisted first: a failed write is an error and leaves nothing.
    let workspace = memory
        .workspaces
        .create(CreateWorkspace {
            name: req.name,
            description: req.description,
        })
        .await
        .map_err(|e| {
            warn!("gRPC CreateWorkspace failed: {}", e);
            status(e)
        })?;

    Ok(Response::new(CreateWorkspaceResponse {
        workspace_id: workspace.id.to_string(),
    }))
}

pub async fn get_workspace(
    memory: &Memory,
    request: Request<GetWorkspaceRequest>,
) -> Result<Response<GetWorkspaceResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetWorkspace: workspace_id={}", req.workspace_id);

    let workspace_id = parse_uuid(&req.workspace_id)?;

    let workspace = memory.workspaces.get(workspace_id).map_err(status)?;

    let info = workspace_to_info(&workspace);

    Ok(Response::new(GetWorkspaceResponse {
        workspace: Some(info),
    }))
}

pub async fn list_workspaces(
    memory: &Memory,
    request: Request<ListWorkspacesRequest>,
) -> Result<Response<ListWorkspacesResponse>, Status> {
    debug!("gRPC ListWorkspaces");

    let include_archived = request.into_inner().include_archived;
    let workspaces = memory.workspaces.list(include_archived);

    let infos: Vec<WorkspaceInfo> = workspaces.iter().map(|ws| workspace_to_info(ws)).collect();

    Ok(Response::new(ListWorkspacesResponse { workspaces: infos }))
}

pub async fn delete_workspace(
    memory: &Memory,
    request: Request<DeleteWorkspaceRequest>,
) -> Result<Response<DeleteWorkspaceResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC DeleteWorkspace: workspace_id={}", req.workspace_id);

    let workspace_id = parse_uuid(&req.workspace_id)?;

    // From storage too (memberships, card, dismissals), not only memory.
    let success = memory
        .workspaces
        .delete(workspace_id)
        .await
        .map_err(status)?;
    Ok(Response::new(DeleteWorkspaceResponse { success }))
}

pub async fn add_session_to_workspace(
    memory: &Memory,
    request: Request<AddSessionToWorkspaceRequest>,
) -> Result<Response<AddSessionToWorkspaceResponse>, Status> {
    let req = request.into_inner();
    debug!(
        "gRPC AddSessionToWorkspace: workspace={}, session={}, role={}",
        req.workspace_id, req.session_id, req.role
    );

    let workspace_id = parse_uuid(&req.workspace_id)?;
    let session_id = parse_uuid(&req.session_id)?;
    // An unset role (proto3 empty) is `related`; an unknown one is invalid.
    let role = match req.role.trim() {
        "" => post_cortex_core::workspace::SessionRole::Related,
        role => parse_session_role("role", role)?,
    };

    memory
        .workspaces
        .add_session(workspace_id, session_id, role)
        .await
        .map_err(status)?;

    Ok(Response::new(AddSessionToWorkspaceResponse {
        success: true,
    }))
}

pub async fn remove_session_from_workspace(
    memory: &Memory,
    request: Request<RemoveSessionFromWorkspaceRequest>,
) -> Result<Response<RemoveSessionFromWorkspaceResponse>, Status> {
    let req = request.into_inner();
    debug!(
        "gRPC RemoveSessionFromWorkspace: workspace={}, session={}",
        req.workspace_id, req.session_id
    );

    let workspace_id = parse_uuid(&req.workspace_id)?;
    let session_id = parse_uuid(&req.session_id)?;

    memory
        .workspaces
        .remove_session(workspace_id, session_id)
        .await
        .map_err(status)?;

    Ok(Response::new(RemoveSessionFromWorkspaceResponse {
        success: true,
    }))
}
