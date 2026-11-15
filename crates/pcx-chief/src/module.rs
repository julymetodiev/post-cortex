// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `chief` module: its manifest (its document kinds, its configuration,
//! the modules it uses, its view, its turns) and its tools and turns over
//! the chief service, whose ports `wiring` makes over its host handle.

use std::sync::Arc;

use async_trait::async_trait;
use pcx_module_api::agent::Turns;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::chief::ChiefConfig;
use post_cortex_proto::chief::chief_server::ChiefServer;
use prost::Name;

use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::settings::Settings;
use crate::turns::ChiefTurns;
use crate::wiring::{Running, Shared};
use crate::{schemas, specs, view};

/// The modules the chief uses: memory (its documents and sessions), the
/// agent (its turns) and the modules its turns reach (a module's turns
/// reach only the modules it uses).
const USES: [&str; 15] = [
    "post-cortex.memory",
    "agent",
    "board",
    "team",
    "skills",
    "tools-fs",
    "tools-shell",
    "tools-git",
    "observe",
    "ops",
    "scheduler",
    "workflow",
    "mail",
    "http",
    "dynamic",
];

/// The `chief` module.
pub struct ChiefModule {
    manifest: Manifest,
    running: Shared,
    turns: Arc<ChiefTurns>,
}

impl ChiefModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        let running = Shared::default();
        Self {
            manifest: Manifest {
                description: "The host’s main agent: answers about the workspace and directs its team through the board.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                config_type: ChiefConfig::full_name(),
                document_kinds: schemas::document_kinds(),
                uses: USES.iter().map(|m| (*m).to_string()).collect(),
                views: view::views(),
                turns: true,
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::CHIEF_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            turns: Arc::new(ChiefTurns {
                shared: Arc::clone(&running),
            }),
            running,
        }
    }
}

impl Default for ChiefModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ChiefModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its schema.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let settings = Settings::of(ctx.config()?)?;
        *self.running.lock() = Some(Running {
            host: ctx.host,
            channel: ctx.channel,
            settings: Arc::new(settings),
        });
        Ok(())
    }

    /// Its `pcx.chief.v1.Chief` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(ChiefServer::new(
            handlers,
        ))))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("chief serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        *self.running.lock() = None;
        Ok(())
    }

    /// Its turns in its own name, as `RunTurn` asks them.
    fn turns(&self) -> Option<Arc<dyn Turns>> {
        Some(Arc::clone(&self.turns) as Arc<dyn Turns>)
    }
}
