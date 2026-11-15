// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `context` module: its manifest (kind `context_engine`, the memory
//! it uses) and its engine over its host handle.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::context::ContextEngine;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::model::ModelMessage;
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleHost, ModuleKind, Outcome, Resource, Scope,
};

use crate::adapters::HostMemory;
use crate::service::Engine;

/// The module's id.
const MODULE_ID: &str = "context";

/// The memory module, whose tools recall and keep summaries.
const MEMORY: &str = "post-cortex.memory";

/// The engine, once the module runs.
#[derive(Default)]
struct Running {
    host: Mutex<Option<Arc<dyn ModuleHost>>>,
    /// The module's host channel, to memory's `pcx.v1`.
    channel: Mutex<Option<GrpcChannel>>,
}

impl Running {
    fn host(&self) -> Result<Arc<dyn ModuleHost>, ModuleError> {
        self.host.lock().clone().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "context has not started",
            )
        })
    }

    fn channel(&self) -> Result<GrpcChannel, ModuleError> {
        self.channel.lock().clone().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "context has not started",
            )
        })
    }
}

#[async_trait]
impl ContextEngine for Running {
    async fn recall(
        &self,
        scope: &Scope,
        session: &str,
        prompt: &str,
    ) -> Result<String, ModuleError> {
        let memory = HostMemory::new(self.channel()?, scope);
        Engine { memory: &memory }.recall(session, prompt).await
    }

    async fn summarize(
        &self,
        scope: &Scope,
        session: &str,
        model: &str,
        messages: &[ModelMessage],
    ) -> Result<String, ModuleError> {
        let host = self.host()?;
        let models = host.model().await?;
        let memory = HostMemory::new(self.channel()?, scope);
        Engine { memory: &memory }
            .summarize(models.as_ref(), scope, session, model, messages)
            .await
    }
}

/// The `context` module.
pub struct ContextModule {
    manifest: Manifest,
    running: Arc<Running>,
}

impl ContextModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Recalls memory for a turn and compacts long transcripts.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::ContextEngine.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                uses: vec![MEMORY.to_string()],
                ..Manifest::default()
            },
            running: Arc::default(),
        }
    }
}

impl Default for ContextModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ContextModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.running.host.lock() = Some(ctx.host);
        *self.running.channel.lock() = Some(ctx.channel);
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("context serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        *self.running.host.lock() = None;
        Ok(())
    }

    fn context_engine(&self) -> Option<Arc<dyn ContextEngine>> {
        Some(Arc::clone(&self.running) as Arc<dyn ContextEngine>)
    }
}
