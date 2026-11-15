// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The webhooks' `pcx.webhooks.v1.Webhooks` service: each RPC maps its
//! request to the service and its answer or failure back.

use pcx_module_api::Scope;
use post_cortex_proto::webhooks::webhooks_server::Webhooks;
use post_cortex_proto::webhooks::{ReceiveRequest, ReceiveResponse};
use tonic::{Request, Response, Status};

use crate::adapters::HostWorld;
use crate::module::{Shared, running};
use crate::service::Service;

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

#[tonic::async_trait]
impl Webhooks for Handlers {
    async fn receive(
        &self,
        request: Request<ReceiveRequest>,
    ) -> Result<Response<ReceiveResponse>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        // One request at a time: a group's incident is added once.
        let _one = running.one.lock().await;
        let world = HostWorld::new(running.channel.clone(), scope);
        let answer = Service::new(&running.config, &world)
            .receive(request.into_inner())
            .await?;
        Ok(Response::new(answer))
    }
}
