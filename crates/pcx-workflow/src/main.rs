// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-workflow`: the workflow module as a module process (the cloud catalog's),
//! serving `pcx.module.v1` on the socket its host names in
//! `PCX_MODULE_SOCKET`.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_workflow::WorkflowModule;

#[tokio::main]
async fn main() -> ExitCode {
    pcx_module_api::main("pcx-workflow", Arc::new(WorkflowModule::new())).await
}
