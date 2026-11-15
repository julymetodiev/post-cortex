// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `scheduler` module: its manifest (its document kinds, its
//! configuration, the modules it uses, its service) and its runner, which
//! starts with it and stops with it.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::scheduler::SchedulerConfig;
use post_cortex_proto::scheduler::scheduler_server::SchedulerServer;
use prost::Name;

use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::runner::Runner;
use crate::specs;

/// The memory module, which keeps the jobs and runs.
const MEMORY: &str = "post-cortex.memory";

/// The board module, which a job's tasks go to.
const BOARD: &str = "board";

/// The workflow module, whose workflows a job may run
/// (docs/modules/workflow.md).
const WORKFLOW: &str = "workflow";

/// The running module, shared by the module and its service; none until
/// it starts.
pub type Shared = Arc<Mutex<Option<Arc<Runner>>>>;

/// The runner of `shared`.
///
/// # Errors
/// `Unavailable` before it starts.
pub fn running(shared: &Shared) -> Result<Arc<Runner>, ModuleError> {
    shared.lock().clone().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::Unavailable,
            "the scheduler has not started",
        )
    })
}

/// The `scheduler` module.
pub struct SchedulerModule {
    manifest: Manifest,
    running: Shared,
}

impl SchedulerModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Jobs at cron times or intervals that run a script under the sandbox or add a board task (a member’s work at a set time).".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Scheduler.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                config_type: SchedulerConfig::full_name(),
                document_kinds: specs::document_kinds(),
                uses: vec![
                    MEMORY.to_string(),
                    BOARD.to_string(),
                    WORKFLOW.to_string(),
                ],
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::SCHEDULER_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for SchedulerModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for SchedulerModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: SchedulerConfig = ctx.config()?;
        if config.workspace_id.trim().is_empty() {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "the scheduler needs workspace_id, where its jobs are kept",
            )
            .with_field("workspace_id"));
        }
        let path = Some(config.path).filter(|p| !p.trim().is_empty());
        let runner = Runner::new(ctx.host, ctx.channel, config.workspace_id, path);
        runner.start();
        if let Some(old) = self.running.lock().replace(runner) {
            old.stop();
        }
        Ok(())
    }

    /// Its `pcx.scheduler.v1.Scheduler` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(
            SchedulerServer::new(handlers),
        )))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("the scheduler serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(runner) = self.running.lock().take() {
            runner.stop();
        }
        Ok(())
    }
}
