// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `webhooks` module: its manifest (its configuration, the modules it
//! uses, its service) and its `pcx.webhooks.v1.Webhooks` service over its
//! host channel, with the configuration checked when it starts.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::module::{Effect, Rpc, ToolVisibility};
use post_cortex_proto::webhooks::WebhooksConfig;
use post_cortex_proto::webhooks::webhooks_server::WebhooksServer;
use prost::Name;

use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::service;

/// The memory module, which keeps the incidents.
const MEMORY: &str = "post-cortex.memory";

/// The board module, which keeps the tasks.
const BOARD: &str = "board";

/// The channel modules, which tell people at once (W29).
const CHANNELS: [&str; 2] = ["discord", "telegram"];

/// The workflow module, which a source may run (docs/modules/workflow.md).
const WORKFLOW: &str = "workflow";

/// The module's service.
const SERVICE: &str = "pcx.webhooks.v1.Webhooks";

/// Its one RPC, which the daemon's route calls as a client: not a tool
/// for agents (visibility `app`).
fn rpcs() -> Vec<Rpc> {
    vec![Rpc {
        method: format!("/{SERVICE}/Receive"),
        tool: "webhook_receive".to_string(),
        visibility: vec![ToolVisibility::App.into()],
        effect: Effect::Write.into(),
    }]
}

/// What a started module works with.
pub struct Running {
    /// Its channel to the board and memory.
    pub channel: GrpcChannel,
    /// Its configuration, checked.
    pub config: WebhooksConfig,
    /// Held while a request is handled: one at a time.
    pub one: tokio::sync::Mutex<()>,
}

/// The running module, shared by the module and its service; none until
/// it starts.
pub type Shared = Arc<Mutex<Option<Arc<Running>>>>;

/// The running module of `shared`.
///
/// # Errors
/// `Unavailable` before it starts.
pub fn running(shared: &Shared) -> Result<Arc<Running>, ModuleError> {
    shared
        .lock()
        .clone()
        .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "webhooks has not started"))
}

/// The `webhooks` module.
pub struct WebhooksModule {
    manifest: Manifest,
    running: Shared,
}

impl WebhooksModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Events from outside (Alertmanager first) become board tasks, one per incident, commented and closed as they fire and resolve.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                config_type: WebhooksConfig::full_name(),
                uses: [BOARD, MEMORY]
                    .into_iter()
                    .chain(CHANNELS)
                    .chain([WORKFLOW])
                    .map(str::to_string)
                    .collect(),
                services: vec![SERVICE.to_string()],
                rpcs: rpcs(),
                descriptors: post_cortex_proto::WEBHOOKS_DESCRIPTOR_SET.to_vec(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for WebhooksModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for WebhooksModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message; the
    /// module checks what the message cannot say (names, kinds, boards).
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config = service::checked(ctx.config()?)?;
        *self.running.lock() = Some(Arc::new(Running {
            channel: ctx.channel,
            config,
            one: tokio::sync::Mutex::new(()),
        }));
        Ok(())
    }

    /// Its `pcx.webhooks.v1.Webhooks` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(
            WebhooksServer::new(handlers),
        )))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("webhooks serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        *self.running.lock() = None;
        Ok(())
    }
}
