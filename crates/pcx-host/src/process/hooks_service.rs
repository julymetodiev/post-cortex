// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx.module.v1.Hooks` on a module process's callback socket: the
//! agent's events through the module's handle, so only for the `agent`
//! slot.

use std::sync::Arc;

use pcx_module_api::{DEFAULT_DEADLINE, ModuleHost, Scope};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_server::Hooks as HooksService;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

/// The agent's events through the module's handle.
pub struct Hooks {
    host: Arc<dyn ModuleHost>,
}

impl Hooks {
    /// The service over `host`, the module's handle.
    pub fn new(host: Arc<dyn ModuleHost>) -> Self {
        Self { host }
    }
}

#[tonic::async_trait]
impl HooksService for Hooks {
    async fn dispatch(
        &self,
        request: Request<wire::DispatchRequest>,
    ) -> Result<Response<wire::DispatchResponse>, Status> {
        let request = request.into_inner();
        let scope = request
            .scope
            .ok_or_else(|| Status::invalid_argument("the request has no scope"))?;
        let scope = Scope::from_wire(scope, CancellationToken::new(), DEFAULT_DEADLINE);
        let kind = wire::EventKind::try_from(request.kind)
            .map_err(|_| Status::invalid_argument("the event's kind is unknown"))?;
        let event = request
            .event
            .ok_or_else(|| Status::invalid_argument("the request has no event"))?;
        Ok(Response::new(
            match self.host.dispatch(&scope, kind, &event).await {
                Ok(outcome) => wire::DispatchResponse {
                    outcome: Some(outcome),
                    error: None,
                },
                Err(error) => wire::DispatchResponse {
                    outcome: None,
                    error: Some(error.into()),
                },
            },
        ))
    }
}
