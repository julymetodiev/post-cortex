// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Serving the bus (`pcx.module.v1.Bus`) on a module's callback socket:
//! [`BusService`] answers through a [`BusSource`], the module's handle.
//! Closing a `Follow` stream stops the feed.

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::bus_server::{Bus as BusRpc, BusServer};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::bus::{Bus, Followed};
use crate::{DEFAULT_DEADLINE, ModuleError, Scope, ThroughHost};

/// Replies of a feed sent before the stream waits.
const QUEUE: usize = 64;

/// Where a served `Bus` finds the bus of its module.
#[async_trait]
pub trait BusSource: Send + Sync + 'static {
    /// The module's bus.
    ///
    /// # Errors
    /// As [`crate::ModuleHost::bus`].
    async fn bus(&self) -> Result<Arc<dyn Bus>, ModuleError>;
}

/// The bus through a module's handle.
#[async_trait]
impl BusSource for ThroughHost {
    async fn bus(&self) -> Result<Arc<dyn Bus>, ModuleError> {
        self.host().bus().await
    }
}

/// `pcx.module.v1.Bus` over `S`.
pub struct BusService<S> {
    source: S,
}

impl<S: BusSource> BusService<S> {
    /// The service over `source`.
    pub fn new(source: S) -> Self {
        Self { source }
    }

    /// Its tonic server.
    pub fn into_server(self) -> BusServer<Self> {
        BusServer::new(self)
    }
}

fn reply(followed: Result<Followed, ModuleError>) -> wire::FollowReply {
    use wire::follow_reply::Reply;
    wire::FollowReply {
        reply: Some(match followed {
            Ok(Followed::Event(event)) => Reply::Event(event),
            Ok(Followed::FellBehind(name)) => Reply::FellBehind(name),
            Err(error) => Reply::Error(error.into()),
        }),
    }
}

#[tonic::async_trait]
impl<S: BusSource> BusRpc for BusService<S> {
    type FollowStream = ReceiverStream<Result<wire::FollowReply, Status>>;

    async fn publish(
        &self,
        request: Request<wire::PublishRequest>,
    ) -> Result<Response<wire::PublishResponse>, Status> {
        let request = request.into_inner();
        let scope = request
            .scope
            .map(|s| Scope::from_wire(s, CancellationToken::new(), DEFAULT_DEADLINE))
            .ok_or_else(|| Status::invalid_argument("the request has no scope"))?;
        let payload = request
            .payload
            .ok_or_else(|| Status::invalid_argument("the request has no payload"))?;
        let published = match self.source.bus().await {
            Ok(bus) => bus.publish(&scope, &request.name, payload).await,
            Err(error) => Err(error),
        };
        Ok(Response::new(match published {
            Ok(id) => wire::PublishResponse { id, error: None },
            Err(error) => wire::PublishResponse {
                id: String::new(),
                error: Some(error.into()),
            },
        }))
    }

    async fn follow(
        &self,
        _request: Request<wire::FollowRequest>,
    ) -> Result<Response<Self::FollowStream>, Status> {
        // A refused follow (the module not running yet, no store) is the
        // call's error, so the module tells it from a feed that ended.
        let mut feed = self
            .source
            .bus()
            .await
            .map_err(Status::from)?
            .follow()
            .await
            .map_err(Status::from)?;
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(async move {
            while let Some(followed) = feed.next().await {
                if tx.send(Ok(reply(followed))).await.is_err() {
                    // The module closed the stream: dropping the feed
                    // stops it.
                    break;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn ack(
        &self,
        request: Request<wire::AckRequest>,
    ) -> Result<Response<wire::AckResponse>, Status> {
        let request = request.into_inner();
        let acked = match self.source.bus().await {
            Ok(bus) => bus.ack(&request.name, request.position).await,
            Err(error) => Err(error),
        };
        Ok(Response::new(wire::AckResponse {
            error: acked.err().map(Into::into),
        }))
    }
}
