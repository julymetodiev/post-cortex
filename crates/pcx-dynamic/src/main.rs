// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx-dynamic`: the dynamic module as a module process (the cloud catalog's),
//! serving `pcx.module.v1` on the socket its host names in `PCX_MODULE_SOCKET`.
//!
//!     pcx-dynamic <launcher socket> <sockets dir> <python> <deno> <imports> [<most modules>]
//!
//! The last comes from who composes the host (the organisation's plan), so no
//! setting of the module can change it; without it there is no limit.

use std::process::ExitCode;
use std::sync::Arc;

use pcx_dynamic::{DynamicModule, LauncherConfig};
use pcx_module_launcher::Runtimes;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [socket, sockets, python, deno, imports, limit @ ..] = args.as_slice() else {
        eprintln!(
            "usage: pcx-dynamic <launcher socket> <sockets dir> <python> <deno> <imports> [<most modules>]"
        );
        return ExitCode::FAILURE;
    };
    let max_modules = match limit {
        [] => None,
        [most] => match most.parse::<usize>() {
            Ok(most) => Some(most),
            Err(e) => {
                eprintln!("the most modules {most:?}: {e}");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: too many arguments");
            return ExitCode::FAILURE;
        }
    };
    let config = LauncherConfig {
        socket: socket.into(),
        sockets: sockets.into(),
        runtimes: Runtimes {
            python: python.into(),
            deno: deno.into(),
            imports: imports.into(),
        },
    };
    pcx_module_api::main(
        "pcx-dynamic",
        Arc::new(DynamicModule::new(config).with_max_modules(max_modules)),
    )
    .await
}
