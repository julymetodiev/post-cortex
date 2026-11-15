// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-discord`: the Discord module as a module process (the cloud catalog's),
//! serving `pcx.module.v1` on the socket its host names in
//! `PCX_MODULE_SOCKET`.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_discord::DiscordModule;

#[tokio::main]
async fn main() -> ExitCode {
    pcx_module_api::main("pcx-discord", Arc::new(DiscordModule::new())).await
}
