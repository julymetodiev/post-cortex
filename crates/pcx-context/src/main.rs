// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-context`: the context engine (recall and summaries) as a module process (the cloud catalog's), serving
//! `pcx.module.v1` and its slot's service on the socket its host names in
//! `PCX_MODULE_SOCKET`, its logs on stderr.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_context::ContextModule;

#[tokio::main]
async fn main() -> ExitCode {
    pcx_module_api::main("pcx-context", Arc::new(ContextModule::new())).await
}
