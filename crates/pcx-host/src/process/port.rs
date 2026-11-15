// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The port a module process of a slot serves on its socket: a model
//! provider's models, a context engine, an agent's (or a module's own)
//! turns.

use std::sync::Arc;

use pcx_module_api::agent::{RemoteTurns, Turns};
use pcx_module_api::context::{ContextEngine, RemoteContext};
use pcx_module_api::model::RemoteModels;
use pcx_module_api::{Manifest, ModuleKind};
use tonic::transport::Channel;

use crate::registry;

/// The port a module process of a slot serves on its socket.
pub enum Port {
    /// It fills no slot with a port.
    None,
    /// A model provider's models, listed at launch and again once it
    /// started.
    Models(Arc<RemoteModels>),
    /// A context engine.
    Context(Arc<dyn ContextEngine>),
    /// An agent's turns, or a module's own (`Manifest.turns`).
    Turns(Arc<dyn Turns>),
}

/// The port the module of `manifest` serves on `channel`, for its slot.
///
/// # Errors
/// Its socket does not answer for it.
pub async fn port(manifest: &Manifest, channel: Channel) -> Result<Port, String> {
    Ok(match registry::kind_of(manifest) {
        Some(ModuleKind::ModelProvider) => Port::Models(Arc::new(
            RemoteModels::connect(channel)
                .await
                .map_err(|e| format!("its models: {e}"))?,
        )),
        Some(ModuleKind::ContextEngine) => Port::Context(Arc::new(
            RemoteContext::connect(channel)
                .await
                .map_err(|e| format!("its context engine: {e}"))?
                .ok_or_else(|| "it serves no context engine".to_string())?,
        )),
        Some(ModuleKind::Agent) => Port::Turns(Arc::new(RemoteTurns::new(channel))),
        _ if manifest.turns => Port::Turns(Arc::new(RemoteTurns::new(channel))),
        _ => Port::None,
    })
}
