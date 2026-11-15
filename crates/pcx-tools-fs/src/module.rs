// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `tools-fs` module: its manifest and its tools over the sandbox its
//! host binds to the policy the operator grants it.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::sandbox::Sandbox;
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleHost, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::tools_fs::files_server::FilesServer;

use crate::handlers::Handlers;
use crate::specs;

/// The module's host handle, shared by the module and its service; none
/// until it starts.
pub type Shared = Arc<Mutex<Option<Arc<dyn ModuleHost>>>>;

/// The sandbox of the running module of `shared`.
///
/// # Errors
/// `FailedPrecondition` before it starts; the host gives it no sandbox.
pub async fn sandbox(shared: &Shared) -> Result<Arc<dyn Sandbox>, ModuleError> {
    let host = shared.lock().clone().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            "tools-fs has not started",
        )
    })?;
    host.sandbox().await
}

/// The `tools-fs` module.
pub struct FsModule {
    manifest: Manifest,
    host: Shared,
}

impl FsModule {
    /// The module's id.
    pub const ID: &'static str = "tools-fs";

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Reads and writes files under the roots it is granted.".to_string(),
                id: Self::ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::Filesystem.into()],
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::TOOLS_FS_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            host: Shared::default(),
        }
    }
}

impl Default for FsModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for FsModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.host.lock() = Some(ctx.host);
        Ok(())
    }

    /// Its `pcx.tools.fs.v1.Files` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.host),
        };
        Some(grpc::routes(tonic::service::Routes::new(FilesServer::new(
            handlers,
        ))))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("tools-fs serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}
