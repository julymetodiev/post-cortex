// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Serving a context engine (`pcx.module.v1.ContextEngine`):
//! [`ContextService`] answers the engine of a [`ContextSource`], a context
//! engine process's own for its host, or the host's through a module's
//! handle for the agent slot.

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::context_engine_server::{
    ContextEngine as ContextRpc, ContextEngineServer,
};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::context::ContextEngine;
use crate::model::ModelMessage;
use crate::{DEFAULT_DEADLINE, ModuleError, ModuleErrorKind, Scope, ThroughHost};

/// Where a served `ContextEngine` finds its engine.
#[async_trait]
pub trait ContextSource: Send + Sync + 'static {
    /// The engine; none when no module fills the slot.
    ///
    /// # Errors
    /// As [`crate::ModuleHost::context`]: the caller is not granted it.
    async fn engine(&self) -> Result<Option<Arc<dyn ContextEngine>>, ModuleError>;
}

/// An engine's own (a context engine process serving its host).
#[async_trait]
impl ContextSource for Arc<dyn ContextEngine> {
    async fn engine(&self) -> Result<Option<Arc<dyn ContextEngine>>, ModuleError> {
        Ok(Some(Arc::clone(self)))
    }
}

/// The host's engine through a module's handle.
#[async_trait]
impl ContextSource for ThroughHost {
    async fn engine(&self) -> Result<Option<Arc<dyn ContextEngine>>, ModuleError> {
        self.host().context().await
    }
}

/// `pcx.module.v1.ContextEngine` over `S`.
pub struct ContextService<S> {
    source: S,
}

impl<S: ContextSource> ContextService<S> {
    /// The service over `source`.
    pub fn new(source: S) -> Self {
        Self { source }
    }

    /// Its tonic server.
    pub fn into_server(self) -> ContextEngineServer<Self> {
        ContextEngineServer::new(self)
    }

    /// The engine; `FailedPrecondition` when none fills the slot.
    async fn engine(&self) -> Result<Arc<dyn ContextEngine>, ModuleError> {
        self.source.engine().await?.ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "no module fills the context_engine slot",
            )
        })
    }
}

fn scope(scope: Option<wire::Scope>) -> Result<Scope, Status> {
    let scope = scope.ok_or_else(|| Status::invalid_argument("the request has no scope"))?;
    Ok(Scope::from_wire(
        scope,
        CancellationToken::new(),
        DEFAULT_DEADLINE,
    ))
}

fn answer(text: Result<String, ModuleError>) -> wire::ContextTextResponse {
    match text {
        Ok(text) => wire::ContextTextResponse { text, error: None },
        Err(error) => wire::ContextTextResponse {
            text: String::new(),
            error: Some(error.into()),
        },
    }
}

#[tonic::async_trait]
impl<S: ContextSource> ContextRpc for ContextService<S> {
    async fn present(
        &self,
        _request: Request<wire::ContextPresentRequest>,
    ) -> Result<Response<wire::ContextPresentResponse>, Status> {
        Ok(Response::new(match self.source.engine().await {
            Ok(engine) => wire::ContextPresentResponse {
                present: engine.is_some(),
                error: None,
            },
            Err(error) => wire::ContextPresentResponse {
                present: false,
                error: Some(error.into()),
            },
        }))
    }

    async fn recall(
        &self,
        request: Request<wire::RecallRequest>,
    ) -> Result<Response<wire::ContextTextResponse>, Status> {
        let request = request.into_inner();
        let scope = scope(request.scope)?;
        let recalled = async {
            self.engine()
                .await?
                .recall(&scope, &request.session, &request.prompt)
                .await
        };
        Ok(Response::new(answer(recalled.await)))
    }

    async fn summarize(
        &self,
        request: Request<wire::SummarizeRequest>,
    ) -> Result<Response<wire::ContextTextResponse>, Status> {
        let request = request.into_inner();
        let scope = scope(request.scope)?;
        let summed = async {
            let messages = request
                .messages
                .into_iter()
                .map(ModelMessage::from_wire)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| ModuleError::new(ModuleErrorKind::InvalidArgument, e.to_string()))?;
            self.engine()
                .await?
                .summarize(&scope, &request.session, &request.model, &messages)
                .await
        };
        Ok(Response::new(answer(summed.await)))
    }
}
