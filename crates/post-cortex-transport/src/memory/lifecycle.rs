// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Workspace lifecycle (update, archive), message deletion and radar
//! dismissals over gRPC. The work is done by capability services; these
//! translate the wire types.

use post_cortex_core::core::dismissal::{RadarDismissal as CoreDismissal, RadarItemKind};
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::UpdateWorkspace;

use tonic::Status;
use uuid::Uuid;

use crate::grpc::status;
use crate::memory::helpers::{parse_uuid, workspace_to_info};
use post_cortex_proto::pb::*;

pub async fn update_workspace(
    memory: &Memory,
    req: UpdateWorkspaceRequest,
) -> Result<UpdateWorkspaceResponse, Status> {
    let workspace_id = parse_uuid(&req.workspace_id)?;
    let workspace = memory
        .workspaces
        .update(
            workspace_id,
            UpdateWorkspace {
                name: req.name,
                description: req.description,
                remote_rerank: req.remote_rerank,
            },
        )
        .await
        .map_err(status)?;
    Ok(UpdateWorkspaceResponse {
        workspace: Some(workspace_to_info(&workspace)),
    })
}

pub async fn set_archived(
    memory: &Memory,
    req: ArchiveWorkspaceRequest,
    archived: bool,
) -> Result<ArchiveWorkspaceResponse, Status> {
    let workspace_id = parse_uuid(&req.workspace_id)?;
    let (workspace, changed) = memory
        .workspaces
        .set_archived(workspace_id, archived)
        .await
        .map_err(status)?;
    Ok(ArchiveWorkspaceResponse {
        workspace: Some(workspace_to_info(&workspace)),
        changed,
    })
}

async fn remaining(memory: &Memory, session_id: Uuid) -> Result<u64, Status> {
    Ok(memory
        .messages
        .stats(session_id)
        .await
        .map_err(status)?
        .count)
}

pub async fn delete_message(
    memory: &Memory,
    req: DeleteMessageRequest,
) -> Result<DeleteMessagesResponse, Status> {
    let session_id = parse_uuid(&req.session_id)?;
    let deleted = memory
        .messages
        .delete(session_id, req.seq)
        .await
        .map_err(status)?;
    Ok(DeleteMessagesResponse {
        deleted: u64::from(deleted),
        remaining: remaining(memory, session_id).await?,
    })
}

pub async fn delete_messages(
    memory: &Memory,
    req: DeleteMessagesRequest,
) -> Result<DeleteMessagesResponse, Status> {
    let session_id = parse_uuid(&req.session_id)?;
    let deleted = memory
        .messages
        .delete_all(session_id)
        .await
        .map_err(status)?;
    Ok(DeleteMessagesResponse {
        deleted: deleted as u64,
        remaining: remaining(memory, session_id).await?,
    })
}

fn parse_kind(kind: &str) -> Result<RadarItemKind, Status> {
    post_cortex_memory::tools::wire::parse("kind", kind).map_err(crate::grpc::status)
}

pub fn dismissal_info(d: &CoreDismissal) -> RadarDismissal {
    RadarDismissal {
        kind: d.kind.as_str().to_string(),
        key: d.key.clone(),
        dismissed_at_unix: d.dismissed_at.timestamp(),
        author: d.author.clone(),
    }
}

pub async fn dismiss(
    memory: &Memory,
    req: DismissRadarItemRequest,
) -> Result<DismissRadarItemResponse, Status> {
    let workspace_id = parse_uuid(&req.workspace_id)?;
    let dismissal = memory
        .views
        .dismiss_radar_item(workspace_id, parse_kind(&req.kind)?, &req.key, req.author)
        .await
        .map_err(status)?;
    Ok(DismissRadarItemResponse {
        dismissal: Some(dismissal_info(&dismissal)),
    })
}

pub async fn restore(
    memory: &Memory,
    req: RestoreRadarItemRequest,
) -> Result<RestoreRadarItemResponse, Status> {
    let workspace_id = parse_uuid(&req.workspace_id)?;
    let existed = memory
        .views
        .restore_radar_item(workspace_id, parse_kind(&req.kind)?, &req.key)
        .await
        .map_err(status)?;
    Ok(RestoreRadarItemResponse { existed })
}

pub async fn list_dismissals(
    memory: &Memory,
    req: ListRadarDismissalsRequest,
) -> Result<ListRadarDismissalsResponse, Status> {
    let workspace_id = parse_uuid(&req.workspace_id)?;
    let dismissals = memory
        .views
        .radar_dismissals(workspace_id)
        .await
        .map_err(status)?;
    Ok(ListRadarDismissalsResponse {
        dismissals: dismissals.iter().map(dismissal_info).collect(),
    })
}
