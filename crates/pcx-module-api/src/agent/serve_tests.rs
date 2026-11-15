// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx.agent.v1.Agent/Models`: the models the agent's turns may name, or
//! why there are none.

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_proto::agent::agent_server::Agent as AgentRpc;
use post_cortex_proto::module as wire;
use tokio::sync::mpsc;
use tonic::Request;

use crate::agent::{TurnEnd, TurnEvent, TurnRequest, Turns, TurnsService, TurnsSource};
use crate::model::{Modality, ModelInfo, ModelKind};
use crate::{ModuleError, ModuleErrorKind, Scope};

/// Turns whose models are `models`, or that choose none.
struct Fixed(Option<Vec<ModelInfo>>);

#[async_trait]
impl Turns for Fixed {
    async fn run(
        &self,
        _scope: &Scope,
        _request: &TurnRequest,
        _events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::Internal, "not run here"))
    }

    async fn models(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        match &self.0 {
            Some(models) => Ok(models.clone()),
            None => Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "no models",
            )),
        }
    }
}

/// A source whose agent is `turns`.
struct Source(Arc<Fixed>);

#[async_trait]
impl TurnsSource for Source {
    async fn turns(&self, _agent: &str) -> Result<Arc<dyn Turns>, ModuleError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn the_agents_models_are_listed_or_why_not() {
    let glm = ModelInfo {
        id: "glm-5.3".into(),
        context_tokens: 200_000,
        output_tokens: 32_768,
        kind: ModelKind::Chat as i32,
        input: vec![Modality::Text as i32],
        output: vec![Modality::Text as i32],
        ..ModelInfo::default()
    };
    let service = TurnsService::new(Source(Arc::new(Fixed(Some(vec![glm.clone()])))));
    let listed = service
        .models(Request::new(wire::ModelsRequest::default()))
        .await
        .expect("models")
        .into_inner();
    assert_eq!(listed.models, vec![glm]);
    assert!(listed.error.is_none());

    let service = TurnsService::new(Source(Arc::new(Fixed(None))));
    let refused = service
        .models(Request::new(wire::ModelsRequest::default()))
        .await
        .expect("models")
        .into_inner();
    assert!(refused.models.is_empty());
    assert!(refused.error.is_some(), "why there are none");
}
