// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The greet module's process: serves [`pcx_greet::Greet`] on the socket
//! the host names in `PCX_MODULE_SOCKET`.

use std::sync::Arc;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    pcx_module_api::main(pcx_greet::ID, Arc::new(pcx_greet::Greet::new())).await
}
