// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The turns a client of the host asks (ARCHITECTURE section 7, the agent):
//! the agent slot's, or those a module runs in its own name. A turn may
//! call any tool its grant allows, so a client that may only read
//! (`Scope.read_only`) is refused it (docs/host.md, effects).

use std::sync::Arc;

use async_trait::async_trait;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, Turns};
use pcx_module_api::model::ModelInfo;
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleKind, Scope};
use tokio::sync::mpsc;

use crate::{Host, Loaded, registry};

/// A module's turns as a client asks them: refused to a caller that may
/// only read.
struct ClientTurns(Arc<dyn Turns>);

#[async_trait]
impl Turns for ClientTurns {
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        if scope.read_only {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                "a caller that may only read cannot run a turn: its tools may change things",
            ));
        }
        self.0.run(scope, request, events).await
    }

    async fn models(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        self.0.models().await
    }
}

fn for_clients(turns: Arc<dyn Turns>) -> Arc<dyn Turns> {
    Arc::new(ClientTurns(turns))
}

impl Host {
    /// The turns of the module in the `agent` slot; none without one.
    #[must_use]
    pub fn turns(&self) -> Option<Arc<dyn Turns>> {
        self.loaded
            .load()
            .modules
            .iter()
            .find(|g| registry::kind_of(g.module().manifest()) == Some(ModuleKind::Agent))
            .and_then(|g| g.module().turns())
            .map(for_clients)
    }

    /// The turns module `id` runs in its own name (`Manifest.turns`, the
    /// chief's); none when no such module runs.
    #[must_use]
    pub fn turns_of(&self, id: &str) -> Option<Arc<dyn Turns>> {
        in_own_name(&self.loaded.load(), id).map(for_clients)
    }
}

/// The turns module `id` of `loaded` runs in its own name
/// (`Manifest.turns`); none when no such module runs.
pub fn in_own_name(loaded: &Loaded, id: &str) -> Option<Arc<dyn Turns>> {
    loaded
        .modules
        .iter()
        .find(|g| g.id() == id && g.module().manifest().turns)
        .and_then(|g| g.module().turns())
}
