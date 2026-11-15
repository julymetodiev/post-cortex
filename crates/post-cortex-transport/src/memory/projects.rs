// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! gRPC side of project resolution (ResolveProject), translated to and from
//! the canonical service, and the read-only lookup (FindProject).

use post_cortex_core::services as svc;
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::ProjectPlacement;
use tonic::Status;

use crate::grpc::status;
use post_cortex_proto::pb;

pub async fn resolve_project(
    memory: &Memory,
    req: pb::ResolveProjectRequest,
) -> Result<pb::ResolveProjectResponse, Status> {
    let response = memory
        .projects
        .resolve(svc::ResolveProjectRequest {
            key: req.key,
            name: req.name,
            workspace: req.workspace,
            client: req.client,
        })
        .await
        .map_err(status)?;
    Ok(pb::ResolveProjectResponse {
        workspace_id: response.workspace_id.to_string(),
        session_id: response.session_id.to_string(),
        created: response.created,
    })
}

/// Where ResolveProject would put `req.key`, creating nothing.
pub fn find_project(memory: &Memory, req: pb::FindProjectRequest) -> pb::FindProjectResponse {
    match memory.projects.find(&req.key, req.workspace.as_deref()) {
        ProjectPlacement::Bound {
            workspace_id,
            session_id,
        } => pb::FindProjectResponse {
            bound: true,
            workspace_id: workspace_id.to_string(),
            session_id: session_id.map(|s| s.to_string()).unwrap_or_default(),
        },
        ProjectPlacement::Joins { workspace_id } => pb::FindProjectResponse {
            bound: false,
            workspace_id: workspace_id.to_string(),
            session_id: String::new(),
        },
        ProjectPlacement::NewWorkspace => pb::FindProjectResponse::default(),
    }
}
