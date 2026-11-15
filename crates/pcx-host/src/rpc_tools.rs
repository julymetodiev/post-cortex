// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The running modules' RPCs as tools (ARCHITECTURE section 7, everything
//! gRPC): each RPC a manifest names as a tool, with its module, called
//! through the host's channel of its caller ([`pcx_module_api::rpc_tool`]).

use std::sync::Arc;

use pcx_module_api::rpc_tool::{self, RpcTool};

use crate::error::HostError;
use crate::policy::Guarded;

/// One RPC of a running module, as a tool.
#[derive(Clone)]
pub struct HostRpc {
    /// The index of its module in the running set.
    pub module: usize,
    /// The tool.
    pub tool: RpcTool,
}

/// The tools of the RPCs every module of `modules` names as tools.
///
/// # Errors
/// `InvalidManifest` for a module whose descriptors do not read, an RPC
/// they do not have, or a visibility that is not one.
pub fn of(modules: &[Arc<Guarded>]) -> Result<Vec<HostRpc>, HostError> {
    let invalid = |index: usize, reason| HostError::InvalidManifest {
        module: modules[index].id().to_string(),
        reason,
    };
    let manifests: Vec<_> = modules.iter().map(|g| g.module().manifest()).collect();
    // One pool for all: a module's answer that carries another's message
    // (an event's payload) maps to JSON.
    let pools = rpc_tool::pools(&manifests).map_err(|(index, reason)| invalid(index, reason))?;
    let mut tools = Vec::new();
    for (index, (manifest, pool)) in manifests.iter().zip(&pools).enumerate() {
        let own = rpc_tool::tools_in(manifest, pool).map_err(|reason| invalid(index, reason))?;
        tools.extend(own.into_iter().map(|tool| HostRpc {
            module: index,
            tool,
        }));
    }
    Ok(tools)
}
