// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx.agent.v1.Agent` over one host (ARCHITECTURE section 7, the agent):
//! the turns of the host's agent (`Host::turns`, so the one that runs now),
//! or of the module the request names (`Host::turns_of`, the chief's),
//! served by `pcx-module-api`'s `TurnsService`; this side only reads a
//! client's request: its scope is a new one, its author the `x-pcx-author`
//! header's (who asks), its workspace the `x-pcx-workspace` header's (where
//! it is asked from), as a module's RPC's, its reach the `x-pcx-reach`
//! header's (docs/connect.md 4a).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use pcx_host::Host;
use pcx_module_api::agent::{TURN_DEADLINE, TurnRequest, Turns, TurnsService, TurnsSource};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope, metadata};
use post_cortex_proto::AUTHOR_HEADER;
use post_cortex_proto::agent as wire;
use post_cortex_proto::agent::agent_server::AgentServer;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Status};

use crate::mcp::author_value;

/// The host's agent, for a client of the host.
pub struct AgentService {
    host: Arc<Host>,
}

impl AgentService {
    /// The service over `host`.
    #[must_use]
    pub fn new(host: Arc<Host>) -> Self {
        Self { host }
    }

    /// The tonic server of the service.
    #[must_use]
    pub fn into_server(self) -> AgentServer<TurnsService<Self>> {
        TurnsService::new(self).into_server()
    }
}

#[async_trait]
impl TurnsSource for AgentService {
    async fn turns(&self, agent: &str) -> Result<Arc<dyn Turns>, ModuleError> {
        if agent.is_empty() {
            return self.host.turns().ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    "no module runs in the agent slot",
                )
            });
        }
        self.host.turns_of(agent).ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::NotFound,
                format!("no module {agent} runs turns here"),
            )
        })
    }

    /// A client's scope: a new one, as the headers name its author, its
    /// workspace, whether it may only read, and its reach (a trusted forwarder's: a gateway, a
    /// connected daemon's tunnel; one that is not text reaches no
    /// workspace), so the turn's every call stays within the asker's reach.
    fn scope(
        &self,
        request: &Request<wire::RunTurnRequest>,
        cancel: CancellationToken,
    ) -> Result<Scope, Status> {
        let workspace_id = request
            .metadata()
            .get(metadata::WORKSPACE)
            .map(|v| {
                v.to_str().map(str::to_string).map_err(|_| {
                    Status::invalid_argument(format!(
                        "the call's {} is not text",
                        metadata::WORKSPACE
                    ))
                })
            })
            .transpose()?
            .filter(|w| !w.is_empty());
        Ok(Scope {
            // A trusted forwarder's mark that the asker may only read: the
            // host refuses its turn (docs/host.md, effects).
            read_only: request.metadata().contains_key(metadata::READ_ONLY),
            // The gateway's: the person the turn is for.
            person: request
                .metadata()
                .get(metadata::PERSON)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            tenant: String::new(),
            profile: String::new(),
            session_id: None,
            workspace_id,
            author: author_value(
                request
                    .metadata()
                    .get(AUTHOR_HEADER)
                    .and_then(|v| v.to_str().ok()),
            ),
            caller_module: None,
            call_depth: 0,
            turn: None,
            reach: request.metadata().get(metadata::REACH).map(|value| {
                value
                    .to_str()
                    .map_or_else(|_| Vec::new(), metadata::reach_of)
            }),
            // An agent's turn never acts with the asker's role (docs/host.md).
            role: None,
            deadline: Instant::now() + TURN_DEADLINE,
            cancel,
        })
    }

    /// A client's turn: who asks is the header's author, not the request's.
    fn asked(&self, request: wire::RunTurnRequest) -> TurnRequest {
        TurnRequest {
            asker: None,
            workspace: None,
            ..TurnRequest::from_wire(request)
        }
    }
}

#[cfg(test)]
mod tests;
