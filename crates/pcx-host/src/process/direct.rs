// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A first-party module's process: started as it is, unconfined, as this
//! host's own binary would be.

use std::process::Stdio;

use async_trait::async_trait;
use pcx_module_api::SOCKET_ENV;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::process::launcher::{Ended, Launch, ModuleLauncher, ModuleProcess, output_lines};

/// Starts a process directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct Direct;

#[async_trait]
impl ModuleLauncher for Direct {
    async fn launch(&self, launch: &Launch) -> Result<Box<dyn ModuleProcess>, String> {
        let child = Command::new(&launch.command)
            .args(&launch.args)
            .env(SOCKET_ENV, &launch.socket)
            // Its own process group: a signal to the host's group (Ctrl-C
            // in a terminal) does not reach it, so the host stops it in
            // order.
            .process_group(0)
            // A host that ends without stopping it takes it along.
            .kill_on_drop(true)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("start {}: {e}", launch.command.display()))?;
        Ok(Box::new(DirectProcess { child }))
    }
}

struct DirectProcess {
    child: Child,
}

#[async_trait]
impl ModuleProcess for DirectProcess {
    fn lines(&mut self) -> Option<mpsc::Receiver<String>> {
        self.child.stderr.take().map(output_lines)
    }

    fn ended(&mut self) -> Result<Option<Ended>, String> {
        self.child
            .try_wait()
            .map(|status| {
                status.map(|s| Ended {
                    how: format!("exited ({s})"),
                    success: s.success(),
                })
            })
            .map_err(|e| format!("wait: {e}"))
    }

    async fn wait(&mut self) -> Ended {
        match self.child.wait().await {
            Ok(status) => Ended {
                how: format!("exited ({status})"),
                success: status.success(),
            },
            Err(e) => Ended {
                how: format!("was lost: {e}"),
                success: false,
            },
        }
    }

    async fn kill(&mut self) -> Result<(), String> {
        self.child.kill().await.map_err(|e| e.to_string())
    }
}
