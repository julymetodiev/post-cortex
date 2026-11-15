// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An out-of-process module's handle on its host: `Host.ListModules`,
//! `Host.ListTools`, the `Sandbox`, `Model`, `Hooks`,
//! `ContextEngine` and `pcx.agent.v1.Agent` services on the callback
//! socket the host names in `StartRequest.host_socket`.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use async_trait::async_trait;
use pcx_module_core::Callback;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_client::HooksClient;
use post_cortex_proto::module::host_client::HostClient;
use tonic::transport::Channel;

use crate::agent::{RemoteTurns, Turns};
use crate::context::{ContextEngine, RemoteContext};
use crate::model::{ModelProvider, RemoteModels};
use crate::rpc_tool::RpcTool;
use crate::sandbox::{RemoteSandbox, Sandbox};
use crate::{
    Event, EventKind, Manifest, ModuleError, ModuleErrorKind, ModuleHost, Outcome, Scope, UsedTool,
    rpc_tool,
};

/// The tools `listed` names (each its module and its RPC's path), built
/// from the descriptors of their modules' `manifests` once per module: a
/// module's tools share one pool, which each tool's messages hold (built
/// once per tool, the chief's 76 tools held 76 pools, past the agent's
/// 256 MB, 2026-11-08).
fn used(
    manifests: &[Manifest],
    listed: impl IntoIterator<Item = (String, String)>,
) -> Result<Vec<UsedTool>, ModuleError> {
    let internal = |reason: String| ModuleError::new(ModuleErrorKind::Internal, reason);
    let all: Vec<&Manifest> = manifests.iter().collect();
    let pools = rpc_tool::pools(&all)
        .map_err(|(index, e)| internal(format!("module {}: {e}", all[index].id)))?;
    let mut built: HashMap<String, Vec<RpcTool>> = HashMap::new();
    let mut used = Vec::new();
    for (module, rpc) in listed {
        let own = match built.entry(module.clone()) {
            Entry::Occupied(own) => own.into_mut(),
            Entry::Vacant(slot) => {
                let index = all.iter().position(|m| m.id == module).ok_or_else(|| {
                    internal(format!(
                        "ListTools named module {module}, which does not run"
                    ))
                })?;
                slot.insert(
                    rpc_tool::tools_in(all[index], &pools[index])
                        .map_err(|e| internal(format!("module {module}: {e}")))?,
                )
            }
        };
        let tool = own
            .iter()
            .find(|t| t.path() == rpc)
            .cloned()
            .ok_or_else(|| internal(format!("module {module} names no tool of rpc {rpc}")))?;
        used.push(UsedTool { module, tool });
    }
    Ok(used)
}

/// The handle over the host's callback socket.
pub struct RemoteHost {
    callback: Callback,
}

impl RemoteHost {
    /// The handle over `socket` (empty: the host offers none). It connects
    /// on its first call.
    pub fn new(socket: &str) -> Self {
        Self {
            callback: Callback::new(socket),
        }
    }

    /// The gRPC channel to the host's callback socket (W23), as
    /// [`Callback::grpc`].
    #[must_use]
    pub fn grpc(&self) -> crate::grpc::GrpcChannel {
        self.callback.grpc()
    }

    fn channel(&self) -> Result<Channel, ModuleError> {
        self.callback.channel()
    }
}

#[async_trait]
impl ModuleHost for RemoteHost {
    async fn manifests(&self) -> Result<Vec<Manifest>, ModuleError> {
        let listed = HostClient::new(self.channel()?)
            .list_modules(wire::ListModulesRequest {})
            .await?
            .into_inner();
        listed
            .modules
            .into_iter()
            .map(|info| {
                info.manifest.ok_or_else(|| {
                    ModuleError::new(
                        ModuleErrorKind::Internal,
                        "ListModules answered a module without its manifest",
                    )
                })
            })
            .collect()
    }

    async fn sandbox(&self) -> Result<Arc<dyn Sandbox>, ModuleError> {
        Ok(Arc::new(RemoteSandbox::connect(self.channel()?).await?))
    }

    async fn model(&self) -> Result<Arc<dyn ModelProvider>, ModuleError> {
        Ok(Arc::new(RemoteModels::connect(self.channel()?).await?))
    }

    async fn bus(&self) -> Result<Arc<dyn crate::bus::Bus>, ModuleError> {
        Ok(Arc::new(crate::bus::RemoteBus::new(self.channel()?)))
    }

    /// The agent's turns behind the callback socket; the host refuses a
    /// module that does not use the agent on its first turn.
    async fn turns(&self) -> Result<Arc<dyn Turns>, ModuleError> {
        Ok(Arc::new(RemoteTurns::new(self.channel()?)))
    }

    /// Module `module`'s own turns behind the callback socket; the host
    /// refuses a module that does not use it on its first turn.
    async fn turns_of(&self, module: &str) -> Result<Arc<dyn Turns>, ModuleError> {
        Ok(Arc::new(RemoteTurns::of(self.channel()?, module)))
    }

    async fn context(&self) -> Result<Option<Arc<dyn ContextEngine>>, ModuleError> {
        Ok(RemoteContext::connect(self.channel()?)
            .await?
            .map(|c| Arc::new(c) as Arc<dyn ContextEngine>))
    }

    /// The tools the host lists for `scope`, as [`used`] builds them.
    async fn tools(&self, scope: &Scope) -> Result<Vec<UsedTool>, ModuleError> {
        let listed = HostClient::new(self.channel()?)
            .list_tools(wire::ListToolsRequest {
                scope: Some(scope.to_wire()),
            })
            .await?
            .into_inner();
        if listed.modules.len() != listed.tools.len() || listed.rpcs.len() != listed.tools.len() {
            return Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "ListTools answered tools without their modules or RPCs",
            ));
        }
        let manifests = self.manifests().await?;
        used(&manifests, listed.modules.into_iter().zip(listed.rpcs))
    }

    async fn dispatch(
        &self,
        scope: &Scope,
        kind: EventKind,
        event: &Event,
    ) -> Result<Outcome, ModuleError> {
        let mut request = tonic::Request::new(wire::DispatchRequest {
            scope: Some(scope.to_wire()),
            kind: kind.into(),
            event: Some(event.clone()),
        });
        request.set_timeout(scope.remaining());
        let answer = HooksClient::new(self.channel()?)
            .dispatch(request)
            .await?
            .into_inner();
        match (answer.outcome, answer.error) {
            (_, Some(error)) => Err(error.into()),
            (Some(outcome), None) => Ok(outcome),
            (None, None) => Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "Dispatch answered without an outcome or an error",
            )),
        }
    }

    async fn report(&self, problem: &str) -> Result<(), ModuleError> {
        let answer = HostClient::new(self.channel()?)
            .report(wire::ReportRequest {
                problem: problem.to_string(),
            })
            .await?
            .into_inner();
        answer.error.map_or(Ok(()), |error| Err(error.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Manifest {
        let rpc = |method: &str, tool: &str| wire::Rpc {
            method: method.into(),
            tool: tool.into(),
            visibility: Vec::new(),
            effect: wire::Effect::Read.into(),
        };
        Manifest {
            id: "board".into(),
            descriptors: post_cortex_proto::BOARD_DESCRIPTOR_SET.to_vec(),
            rpcs: vec![
                rpc("/pcx.board.v1.Boards/ListBoards", "board_list"),
                rpc("/pcx.board.v1.Boards/GetTask", "task_get"),
            ],
            ..Manifest::default()
        }
    }

    #[test]
    fn a_modules_tools_share_one_pool_and_an_unknown_one_is_refused() {
        let manifests = [board()];
        let listed = |rpcs: &[&str]| {
            rpcs.iter()
                .map(|rpc| ("board".to_string(), (*rpc).to_string()))
                .collect::<Vec<_>>()
        };
        let tools = used(
            &manifests,
            listed(&[
                "/pcx.board.v1.Boards/GetTask",
                "/pcx.board.v1.Boards/ListBoards",
            ]),
        )
        .unwrap();
        let names: Vec<_> = tools.iter().map(|t| t.tool.spec.name.as_str()).collect();
        assert_eq!(names, ["task_get", "board_list"]);
        assert!(
            tools[0].tool.request().parent_pool() == tools[1].tool.request().parent_pool(),
            "every tool of a module is built from one pool"
        );

        let refused = |listed| used(&manifests, listed).unwrap_err().message;
        assert!(refused(listed(&["/pcx.board.v1.Boards/AddTask"])).contains("names no tool"));
        assert!(
            refused(vec![("mail".into(), "/pcx.mail.v1.Mail/Send".into())])
                .contains("does not run")
        );
    }
}
