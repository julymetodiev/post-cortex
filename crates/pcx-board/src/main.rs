// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-board`: the board as a module process (the cloud catalog's), serving
//! `pcx.module.v1` on the socket its host names in `PCX_MODULE_SOCKET`.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_board::BoardModule;

#[tokio::main]
async fn main() -> ExitCode {
    pcx_module_api::main("pcx-board", Arc::new(BoardModule::new())).await
}
