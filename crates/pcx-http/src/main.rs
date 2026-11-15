// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-http`: the http module as a module process (the cloud catalog's),
//! serving `pcx.module.v1` on the socket its host names in
//! `PCX_MODULE_SOCKET`. `--public-only` (the cloud's) refuses a
//! configuration that reaches private networks.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_http::HttpModule;

#[tokio::main]
async fn main() -> ExitCode {
    let public_only = std::env::args().any(|a| a == HttpModule::PUBLIC_ONLY);
    pcx_module_api::main("pcx-http", Arc::new(HttpModule::new(public_only))).await
}
