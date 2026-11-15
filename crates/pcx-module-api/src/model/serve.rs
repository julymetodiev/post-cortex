// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Serving models (`pcx.module.v1.Model`): [`ModelService`] answers the
//! models of a [`ModelSource`], a provider process's own for its host, or
//! the host's through a module's handle for the agent and context
//! engine slots. Closing a `Generate` stream cancels the call.

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::model_server::{Model as ModelRpc, ModelServer};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::model::{ModelEvent, ModelFailure, ModelProvider, ModelRequest};
use crate::{DEFAULT_DEADLINE, ModuleError, Scope, ThroughHost};

/// Replies of an answer sent before the stream waits.
const QUEUE: usize = 64;

/// Where a served `Model` finds its models.
#[async_trait]
pub trait ModelSource: Send + Sync + 'static {
    /// The provider of the models to serve.
    ///
    /// # Errors
    /// As [`crate::ModuleHost::model`]: the caller is not granted models.
    async fn provider(&self) -> Result<Arc<dyn ModelProvider>, ModuleError>;
}

/// A provider's own models (a provider process serving its host).
#[async_trait]
impl ModelSource for Arc<dyn ModelProvider> {
    async fn provider(&self) -> Result<Arc<dyn ModelProvider>, ModuleError> {
        Ok(Arc::clone(self))
    }
}

/// The host's models through a module's handle.
#[async_trait]
impl ModelSource for ThroughHost {
    async fn provider(&self) -> Result<Arc<dyn ModelProvider>, ModuleError> {
        self.host().model().await
    }
}

/// `pcx.module.v1.Model` over `S`.
pub struct ModelService<S> {
    source: S,
}

impl<S: ModelSource> ModelService<S> {
    /// The service over `source`.
    pub fn new(source: S) -> Self {
        Self { source }
    }

    /// Its tonic server.
    pub fn into_server(self) -> ModelServer<Self> {
        ModelServer::new(self)
    }
}

fn failure(failure: ModelFailure) -> wire::GenerateReply {
    wire::GenerateReply {
        event: Some(wire::generate_reply::Event::Failure(failure.to_wire())),
    }
}

#[tonic::async_trait]
impl<S: ModelSource> ModelRpc for ModelService<S> {
    type GenerateStream = ReceiverStream<Result<wire::GenerateReply, Status>>;

    async fn models(
        &self,
        request: Request<wire::ModelsRequest>,
    ) -> Result<Response<wire::ModelsResponse>, Status> {
        let listed = match self.source.provider().await {
            Ok(models) if request.get_ref().settled => models.listed().await,
            Ok(models) => Ok(models.models()),
            Err(error) => Err(error),
        };
        Ok(Response::new(match listed {
            Ok(models) => wire::ModelsResponse {
                models,
                error: None,
            },
            Err(error) => wire::ModelsResponse {
                models: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }

    async fn generate(
        &self,
        request: Request<wire::GenerateRequest>,
    ) -> Result<Response<Self::GenerateStream>, Status> {
        let mut request = request.into_inner();
        let scope = request
            .scope
            .take()
            .ok_or_else(|| Status::invalid_argument("the request has no scope"))?;
        let cancel = CancellationToken::new();
        let scope = Scope::from_wire(scope, cancel.clone(), DEFAULT_DEADLINE);
        let models = self.source.provider().await.map_err(Status::from)?;
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(async move {
            // The caller closing the stream cancels the call.
            let _cancel_on_close = cancel.drop_guard();
            let started = match ModelRequest::from_wire(request) {
                Ok(asked) => models.generate(&scope, asked).await,
                Err(refused) => Err(refused),
            };
            let mut run = match started {
                Ok(run) => run,
                Err(refused) => {
                    if tx.send(Ok(failure(refused))).await.is_err() {
                        tracing::debug!("a model stream closed before its failure");
                    }
                    return;
                }
            };
            while let Some(event) = run.next().await {
                let last = !matches!(
                    event,
                    Ok(ModelEvent::TextDelta(_)
                        | ModelEvent::ThinkingDelta(_)
                        | ModelEvent::ToolUse { .. })
                );
                let reply = event.and_then(ModelEvent::to_wire).unwrap_or_else(failure);
                if tx.send(Ok(reply)).await.is_err() || last {
                    return;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
