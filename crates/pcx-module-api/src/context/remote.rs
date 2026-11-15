// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module process's context engine: `pcx.module.v1.ContextEngine` on its
//! callback socket.

use async_trait::async_trait;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::context_engine_client::ContextEngineClient;
use tonic::transport::Channel;

use crate::context::ContextEngine;
use crate::model::ModelMessage;
use crate::{ModuleError, ModuleErrorKind, Scope};

/// The context engine behind the host's callback socket.
pub struct RemoteContext {
    client: ContextEngineClient<Channel>,
}

impl RemoteContext {
    /// The context engine over `channel`; none when no module fills the
    /// slot.
    ///
    /// # Errors
    /// The host refuses this module the context engine, or cannot be
    /// reached.
    pub async fn connect(channel: Channel) -> Result<Option<Self>, ModuleError> {
        let mut client = ContextEngineClient::new(channel);
        let answer = client
            .present(wire::ContextPresentRequest {})
            .await?
            .into_inner();
        if let Some(error) = answer.error {
            return Err(error.into());
        }
        Ok(answer.present.then_some(Self { client }))
    }
}

/// The text of an answer, or its error.
fn text(answer: wire::ContextTextResponse) -> Result<String, ModuleError> {
    match answer.error {
        Some(error) => Err(error.into()),
        None => Ok(answer.text),
    }
}

#[async_trait]
impl ContextEngine for RemoteContext {
    async fn recall(
        &self,
        scope: &Scope,
        session: &str,
        prompt: &str,
    ) -> Result<String, ModuleError> {
        let mut request = tonic::Request::new(wire::RecallRequest {
            scope: Some(scope.to_wire()),
            session: session.to_string(),
            prompt: prompt.to_string(),
        });
        request.set_timeout(scope.remaining());
        text(self.client.clone().recall(request).await?.into_inner())
    }

    async fn summarize(
        &self,
        scope: &Scope,
        session: &str,
        model: &str,
        messages: &[ModelMessage],
    ) -> Result<String, ModuleError> {
        let messages = messages
            .iter()
            .cloned()
            .map(ModelMessage::to_wire)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ModuleError::new(ModuleErrorKind::InvalidArgument, e.to_string()))?;
        let mut request = tonic::Request::new(wire::SummarizeRequest {
            scope: Some(scope.to_wire()),
            session: session.to_string(),
            model: model.to_string(),
            messages,
        });
        request.set_timeout(scope.remaining());
        text(self.client.clone().summarize(request).await?.into_inner())
    }
}
