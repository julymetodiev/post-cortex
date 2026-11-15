// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Every workspace of the host, for a module's own work across them (the
//! jobs due, the work a stop cut short, the runs left open): memory's
//! `pcx.v1` workspaces over the host channel (W23).

use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::pb::ListWorkspacesRequest;
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use tonic::Request;

/// The id of every workspace, asked as `scope`'s author within its
/// deadline, naming no workspace.
///
/// # Errors
/// Memory refused or failed.
pub async fn every_workspace(
    channel: &GrpcChannel,
    scope: &Scope,
) -> Result<Vec<String>, ModuleError> {
    let every = Scope {
        workspace_id: None,
        ..scope.clone()
    };
    let mut request = Request::new(ListWorkspacesRequest::default());
    every.to_metadata(request.metadata_mut())?;
    let listed = PostCortexClient::new(channel.clone())
        .list_workspaces(request)
        .await?
        .into_inner();
    Ok(listed
        .workspaces
        .into_iter()
        .map(|workspace| workspace.workspace_id)
        .collect())
}
