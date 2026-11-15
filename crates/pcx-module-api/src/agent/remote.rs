// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Turns behind a socket (`pcx.agent.v1.Agent`): a module's, on its host's
//! callback socket, or an agent process's, on its own socket for its host.
//! Cancelling the scope cancels the turn on the server (`CancelTurn`), and
//! its end, `cancelled` with its steps and usage, is still read.

use async_trait::async_trait;
use post_cortex_proto::agent as wire;
use post_cortex_proto::agent::agent_client::AgentClient;
use post_cortex_proto::module as models_wire;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use crate::agent::wire::Reply;
use crate::agent::{TurnEnd, TurnEvent, TurnRequest, Turns};
use crate::model::{ModelInfo, checked};
use crate::{ModuleError, ModuleErrorKind, Scope};

/// The turns behind `channel`.
pub struct RemoteTurns {
    client: AgentClient<Channel>,
    /// Whose turns the server is asked (`RunTurnRequest.agent`); empty: its
    /// own, or the agent's.
    agent: String,
}

impl RemoteTurns {
    /// The turns over `channel` (it connects on the first turn).
    #[must_use]
    pub fn new(channel: Channel) -> Self {
        Self::of(channel, "")
    }

    /// The turns module `module` runs in its own name, asked of the host
    /// over `channel` (a module's callback socket).
    #[must_use]
    pub fn of(channel: Channel, module: &str) -> Self {
        Self {
            client: AgentClient::new(channel),
            agent: module.to_string(),
        }
    }

    /// Cancel the server's turn `turn_id`.
    async fn cancel(&self, turn_id: &str) -> Result<(), ModuleError> {
        let answer = self
            .client
            .clone()
            .cancel_turn(wire::CancelTurnRequest {
                turn_id: turn_id.to_string(),
            })
            .await?
            .into_inner();
        if !answer.running {
            tracing::debug!(turn_id, "the turn had ended before its cancel");
        }
        Ok(())
    }
}

#[async_trait]
impl Turns for RemoteTurns {
    async fn models(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        let answer = self
            .client
            .clone()
            .models(models_wire::ModelsRequest::default())
            .await?
            .into_inner();
        if let Some(error) = answer.error {
            return Err(error.into());
        }
        Ok(checked(answer.models)?)
    }

    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        let mut call = tonic::Request::new(wire::RunTurnRequest {
            agent: self.agent.clone(),
            ..request.to_wire(Some(scope.to_wire()))
        });
        call.set_timeout(scope.remaining());
        let mut replies = self.client.clone().run_turn(call).await?.into_inner();
        // The server's id of the turn, once it says; whether it was asked
        // to cancel.
        let mut turn: Option<String> = None;
        let mut cancelled = false;
        loop {
            let next = tokio::select! {
                () = scope.cancel.cancelled(), if !cancelled && turn.is_some() => {
                    cancelled = true;
                    if let Some(id) = &turn {
                        self.cancel(id).await?;
                    }
                    continue;
                }
                next = replies.message() => next?,
            };
            let Some(reply) = next else {
                return Err(ModuleError::new(
                    ModuleErrorKind::Unavailable,
                    "the agent's stream closed before the turn ended",
                ));
            };
            match Reply::from_wire(reply)? {
                Reply::Started(id) => turn = Some(id),
                Reply::Event(event) => {
                    if events.send(event).await.is_err() {
                        tracing::debug!("a remote turn's reader left; the turn goes on");
                    }
                }
                Reply::End(end) => return Ok(end),
            }
        }
    }
}
