// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-agent`: the agent (turns with models and tools) as a module process (the cloud catalog's), serving
//! `pcx.module.v1` and its slot's service on the socket its host names in
//! `PCX_MODULE_SOCKET`, its logs on stderr.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_agent::AgentModule;

#[tokio::main]
async fn main() -> ExitCode {
    pcx_module_api::main("pcx-agent", Arc::new(AgentModule::new())).await
}
