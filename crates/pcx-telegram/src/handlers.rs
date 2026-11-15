// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The channel's `pcx.channel.v1.Channel` service: `Send` maps its request
//! to the service and its answer or failure back.

use post_cortex_proto::channel::channel_server::Channel;
use post_cortex_proto::channel::{SendRequest, SendResponse};
use tonic::{Request, Response, Status};

use crate::module::{Shared, running};

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

#[tonic::async_trait]
impl Channel for Handlers {
    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        let message_id = running
            .service()
            .send(&request.destination, &request.text)
            .await?;
        Ok(Response::new(SendResponse { message_id }))
    }
}
