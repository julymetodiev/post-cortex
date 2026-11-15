// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Memory over the module's host channel, in the call's scope (W23):
//! recall through `pcx.v1` `AssembleContext`, a summary kept through
//! `UpdateContext`.

use async_trait::async_trait;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use post_cortex_proto::pb::{
    AssembleContextRequest, ContextContent, EntityInfo, RelationInfo, UpdateContextRequest,
};
use tonic::Request;

use crate::ports::Memory;

/// Tokens a recall may take.
const RECALL_TOKENS: u32 = 2000;

/// Memory for one call.
pub struct HostMemory<'a> {
    channel: GrpcChannel,
    scope: &'a Scope,
}

impl<'a> HostMemory<'a> {
    /// Memory over the module's host `channel`, in `scope`.
    pub fn new(channel: GrpcChannel, scope: &'a Scope) -> Self {
        Self { channel, scope }
    }

    /// `message` as a request in the call's scope.
    fn request<T>(&self, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        self.scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

fn entity(name: &str, entity_type: &str) -> EntityInfo {
    EntityInfo {
        name: name.to_string(),
        entity_type: entity_type.to_string(),
    }
}

#[async_trait]
impl Memory for HostMemory<'_> {
    async fn recall(&self, session: &str, query: &str) -> Result<(usize, String), ModuleError> {
        let assembled = PostCortexClient::new(self.channel.clone())
            .assemble_context(self.request(AssembleContextRequest {
                session_id: session.to_string(),
                query: query.to_string(),
                token_budget: RECALL_TOKENS,
                ..AssembleContextRequest::default()
            })?)
            .await?
            .into_inner();
        Ok((assembled.items.len(), assembled.formatted_text))
    }

    async fn keep_summary(&self, session: &str, summary: &str) -> Result<(), ModuleError> {
        let conversation = format!("conversation {session}");
        let agent = "pcx agent";
        PostCortexClient::new(self.channel.clone())
            .update_context(self.request(UpdateContextRequest {
                session_id: session.to_string(),
                interaction_type: "qa".to_string(),
                content: Some(ContextContent {
                    title: "What has the conversation covered so far?".to_string(),
                    description: summary.to_string(),
                    ..ContextContent::default()
                }),
                entities: vec![
                    entity(&conversation, "concept"),
                    entity(agent, "technology"),
                ],
                relations: vec![RelationInfo {
                    from_entity: conversation.clone(),
                    to_entity: agent.to_string(),
                    relation_type: "related_to".to_string(),
                    context: "a summary of the agent's conversation".to_string(),
                }],
                ..UpdateContextRequest::default()
            })?)
            .await?;
        Ok(())
    }
}
