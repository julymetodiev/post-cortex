// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The module's link to its server (docs/modules/discord.md, connecting a
//! server): read after the module started, never while it starts, so a
//! host starts whatever Discord answers; kept once read, and until then
//! why it is not.

use std::sync::Arc;

use parking_lot::Mutex;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::discord::{DiscordConfig, ServerStatus};

use crate::domain::rights::Server;
use crate::ports::Rest;
use crate::service::server;

/// The server as the module read it, or why it has not.
pub struct Linked {
    server: Mutex<Result<Arc<Server>, ModuleError>>,
}

impl Default for Linked {
    fn default() -> Self {
        Self {
            server: Mutex::new(Err(ModuleError::new(
                ModuleErrorKind::Unavailable,
                "discord has not read its server yet",
            ))),
        }
    }
}

impl Linked {
    /// The server as it was read.
    ///
    /// # Errors
    /// Why it was not: not yet (`Unavailable`), what the bot lacks there
    /// (`FailedPrecondition`), or how the API failed.
    pub fn server(&self) -> Result<Arc<Server>, ModuleError> {
        self.server.lock().clone()
    }

    /// The server, read over `rest` as `config` needs it unless it was
    /// read; what the read answers is kept.
    ///
    /// # Errors
    /// As [`server`].
    pub async fn read(
        &self,
        config: &DiscordConfig,
        rest: &dyn Rest,
    ) -> Result<Arc<Server>, ModuleError> {
        if let Ok(read) = self.server() {
            return Ok(read);
        }
        let read = server(config, rest).await.map(Arc::new);
        *self.server.lock() = read.clone();
        read
    }

    /// Whether the module works with its server, and why not.
    pub fn status(&self) -> ServerStatus {
        match self.server() {
            Ok(_) => ServerStatus {
                connected: true,
                problem: String::new(),
            },
            Err(e) => ServerStatus {
                connected: false,
                problem: e.message,
            },
        }
    }
}
