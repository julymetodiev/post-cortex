// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's sessions in memory: a new session in its workspace, through
//! memory's `pcx.v1` sessions and workspaces over the host channel (W23).

use async_trait::async_trait;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use post_cortex_proto::pb::{AddSessionToWorkspaceRequest, CreateSessionRequest};
use tonic::Request;

use crate::Sessions;

/// The role of a module's session in its workspace.
const ROLE: &str = "related";

/// A module's sessions in `workspace`, for one call.
pub struct HostSessions<'a> {
    channel: GrpcChannel,
    scope: &'a Scope,
    workspace: &'a str,
}

impl<'a> HostSessions<'a> {
    /// The sessions over the module's host `channel`, in `scope`, in
    /// `workspace`.
    #[must_use]
    pub fn new(channel: GrpcChannel, scope: &'a Scope, workspace: &'a str) -> Self {
        Self {
            channel,
            scope,
            workspace,
        }
    }

    /// `message` as a request in the call's scope.
    fn request<T>(&self, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        self.scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

#[async_trait]
impl Sessions for HostSessions<'_> {
    /// A new session named `name`, in the workspace (role `related`).
    async fn create(&self, name: &str) -> Result<String, ModuleError> {
        let mut client = PostCortexClient::new(self.channel.clone());
        let created = client
            .create_session(self.request(CreateSessionRequest {
                name: name.to_string(),
                ..CreateSessionRequest::default()
            })?)
            .await?
            .into_inner();
        client
            .add_session_to_workspace(self.request(AddSessionToWorkspaceRequest {
                workspace_id: self.workspace.to_string(),
                session_id: created.session_id.clone(),
                role: ROLE.to_string(),
            })?)
            .await?;
        Ok(created.session_id)
    }
}
