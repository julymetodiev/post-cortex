// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-models`: every model provider as one module process (the cloud
//! catalog's), serving `pcx.module.v1` and its slot's service on the
//! socket its host names in `PCX_MODULE_SOCKET`, its logs on stderr.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_models::ModelsModule;

#[tokio::main]
async fn main() -> ExitCode {
    pcx_module_api::main("pcx-models", Arc::new(ModelsModule::new())).await
}
