// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module process's bus: `pcx.module.v1.Bus` on its callback socket.
//! Dropping a feed closes its stream.

use async_trait::async_trait;
use pbjson_types::Any;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::bus_client::BusClient;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use crate::bus::{Bus, EventFeed, Followed};
use crate::{ModuleError, ModuleErrorKind, Scope};

/// Replies of a feed read before the reader waits.
const QUEUE: usize = 64;

/// The bus behind the host's callback socket.
pub struct RemoteBus {
    client: BusClient<Channel>,
}

impl RemoteBus {
    /// The bus over `channel`.
    #[must_use]
    pub fn new(channel: Channel) -> Self {
        Self {
            client: BusClient::new(channel),
        }
    }
}

fn followed(reply: wire::FollowReply) -> Result<Followed, ModuleError> {
    use wire::follow_reply::Reply;
    match reply.reply {
        Some(Reply::Event(event)) => Ok(Followed::Event(event)),
        Some(Reply::FellBehind(name)) => Ok(Followed::FellBehind(name)),
        Some(Reply::Error(error)) => Err(error.into()),
        None => Err(ModuleError::new(
            ModuleErrorKind::Internal,
            "the host answered a follow reply with nothing in it",
        )),
    }
}

#[async_trait]
impl Bus for RemoteBus {
    async fn publish(
        &self,
        scope: &Scope,
        name: &str,
        payload: Any,
    ) -> Result<String, ModuleError> {
        let answer = self
            .client
            .clone()
            .publish(wire::PublishRequest {
                scope: Some(scope.to_wire()),
                name: name.to_string(),
                payload: Some(payload),
            })
            .await?
            .into_inner();
        match answer.error {
            Some(error) => Err(error.into()),
            None => Ok(answer.id),
        }
    }

    async fn follow(&self) -> Result<EventFeed, ModuleError> {
        let mut stream = self
            .client
            .clone()
            .follow(wire::FollowRequest {})
            .await?
            .into_inner();
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(async move {
            loop {
                let next = match stream.message().await {
                    Ok(Some(reply)) => followed(reply),
                    Ok(None) => break,
                    Err(status) => Err(status.into()),
                };
                let ended = next.is_err();
                if tx.send(next).await.is_err() || ended {
                    break;
                }
            }
        });
        Ok(EventFeed::new(rx))
    }

    async fn ack(&self, name: &str, position: u64) -> Result<(), ModuleError> {
        let answer = self
            .client
            .clone()
            .ack(wire::AckRequest {
                name: name.to_string(),
                position,
            })
            .await?
            .into_inner();
        match answer.error {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }
}
