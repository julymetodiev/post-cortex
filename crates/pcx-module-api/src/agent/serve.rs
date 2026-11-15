// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Serving turns (`pcx.agent.v1.Agent`): [`TurnsService`] runs the turns
//! of a [`TurnsSource`], each in the scope the source reads from its
//! request, streams `started`, its events and its end, and cancels a turn
//! when `CancelTurn` names it or its reader leaves. Every server of turns
//! is this one: an agent process for its host, a host for a module that
//! uses the agent, the daemon for its clients.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use post_cortex_proto::agent as wire;
use post_cortex_proto::agent::agent_server::{Agent as AgentRpc, AgentServer};
use post_cortex_proto::module as models_wire;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::agent::wire::started;
use crate::agent::{TurnEvent, TurnRequest, Turns};
use crate::{ModuleError, ModuleErrorKind, Scope, ThroughHost};

/// Longest a turn may run when its request sets no deadline.
pub const TURN_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// Replies sent before the stream waits for its reader.
const QUEUE: usize = 256;

/// Where a served `Agent` finds its turns, and the scope a request runs in.
#[async_trait]
pub trait TurnsSource: Send + Sync + 'static {
    /// The turns to run of `agent`, as the request names it (empty: the
    /// agent's).
    ///
    /// # Errors
    /// As [`crate::ModuleHost::turns`]: no agent, or one not granted; no
    /// such module running turns.
    async fn turns(&self, agent: &str) -> Result<Arc<dyn Turns>, ModuleError>;

    /// The scope `request` runs in, cancelled through `cancel`: by default
    /// the one it carries (between a host and a module).
    ///
    /// # Errors
    /// `InvalidArgument` for a request without one.
    fn scope(
        &self,
        request: &Request<wire::RunTurnRequest>,
        cancel: CancellationToken,
    ) -> Result<Scope, Status> {
        let scope = request
            .get_ref()
            .scope
            .clone()
            .ok_or_else(|| Status::invalid_argument("the request has no scope"))?;
        Ok(Scope::from_wire(scope, cancel, TURN_DEADLINE))
    }

    /// The turn `request` asks: by default all it carries (between a host
    /// and a module).
    fn asked(&self, request: wire::RunTurnRequest) -> TurnRequest {
        TurnRequest::from_wire(request)
    }
}

/// A module's own turns (an agent or module process serving its host): a
/// request that names whose turn it is is refused, it is always the
/// server's own.
#[async_trait]
impl TurnsSource for Arc<dyn Turns> {
    async fn turns(&self, agent: &str) -> Result<Arc<dyn Turns>, ModuleError> {
        if !agent.is_empty() {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("a module serves its own turns, not those of {agent}"),
            ));
        }
        Ok(Arc::clone(self))
    }
}

/// The turns a module asks through its handle (a host serving a module):
/// the agent's when the request names none, else those the module it names
/// runs in its own name.
#[async_trait]
impl TurnsSource for ThroughHost {
    async fn turns(&self, agent: &str) -> Result<Arc<dyn Turns>, ModuleError> {
        if agent.is_empty() {
            self.host().turns().await
        } else {
            self.host().turns_of(agent).await
        }
    }
}

/// The running turns of a server, by id.
type Running = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// `pcx.agent.v1.Agent` over `S`.
pub struct TurnsService<S> {
    source: S,
    running: Running,
}

impl<S: TurnsSource> TurnsService<S> {
    /// The service over `source`.
    pub fn new(source: S) -> Self {
        Self {
            source,
            running: Running::default(),
        }
    }

    /// Its tonic server.
    pub fn into_server(self) -> AgentServer<Self> {
        AgentServer::new(self)
    }
}

#[tonic::async_trait]
impl<S: TurnsSource> AgentRpc for TurnsService<S> {
    type RunTurnStream = ReceiverStream<Result<wire::TurnReply, Status>>;

    /// No agent, or a turn that fails before its events, is a gRPC error;
    /// how a running turn ended is its last reply.
    async fn run_turn(
        &self,
        request: Request<wire::RunTurnRequest>,
    ) -> Result<Response<Self::RunTurnStream>, Status> {
        let cancel = CancellationToken::new();
        let scope = self.source.scope(&request, cancel.clone())?;
        let turns = self
            .source
            .turns(&request.get_ref().agent)
            .await
            .map_err(Status::from)?;
        let asked = self.source.asked(request.into_inner());
        let id = uuid::Uuid::new_v4().to_string();
        self.running.lock().insert(id.clone(), cancel.clone());
        let running = Arc::clone(&self.running);
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(async move {
            if tx.send(Ok(started(id.clone()))).await.is_err() {
                running.lock().remove(&id);
                return;
            }
            let (events, mut told) = mpsc::channel::<TurnEvent>(QUEUE);
            let forward = tx.clone();
            // Each event as it happens; the reader gone cancels the turn.
            let relay = tokio::spawn(async move {
                while let Some(event) = told.recv().await {
                    if forward.send(Ok(event.to_wire())).await.is_err() {
                        cancel.cancel();
                        return;
                    }
                }
            });
            let ended = turns.run(&scope, &asked, &events).await;
            running.lock().remove(&id);
            drop(events);
            if let Err(e) = relay.await {
                tracing::warn!(error = %e, "the turn's relay stopped");
            }
            let last = match ended {
                Ok(end) => Ok(end.to_wire()),
                Err(error) => Err(Status::from(error)),
            };
            if tx.send(last).await.is_err() {
                tracing::debug!("a turn ended after its reader left");
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn models(
        &self,
        _request: Request<models_wire::ModelsRequest>,
    ) -> Result<Response<models_wire::ModelsResponse>, Status> {
        let models = match self.source.turns("").await {
            Ok(turns) => turns.models().await,
            Err(error) => Err(error),
        };
        Ok(Response::new(match models {
            Ok(models) => models_wire::ModelsResponse {
                models,
                error: None,
            },
            Err(error) => models_wire::ModelsResponse {
                models: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }

    async fn cancel_turn(
        &self,
        request: Request<wire::CancelTurnRequest>,
    ) -> Result<Response<wire::CancelTurnResponse>, Status> {
        let id = request.into_inner().turn_id;
        let cancel = self.running.lock().get(&id).cloned();
        if let Some(cancel) = &cancel {
            cancel.cancel();
        }
        Ok(Response::new(wire::CancelTurnResponse {
            running: cancel.is_some(),
        }))
    }
}
