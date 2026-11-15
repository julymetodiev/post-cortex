// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The runtime of an out-of-process Rust module: [`serve`] runs a
//! [`Module`] as a `pcx-module-core` [`Process`], with the service of the
//! slot it fills beside it: its models (`Model`), its context engine
//! (`ContextEngine`) or its turns (`pcx.agent.v1.Agent`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use pcx_module_core::{Process, SOCKET_ENV, ServeError, Start};

use crate::agent::TurnsService;
use crate::context::ContextService;
use crate::model::ModelService;
use crate::remote::RemoteHost;
use crate::{
    Event, HostContext, Manifest, Module, ModuleError, Outcome, Prompt, PromptContent, Resource,
    Scope,
};

/// A module process's `main`: its logs on stderr (as
/// [`pcx_module_core::logs`]); `module` served on the socket the host
/// names in [`SOCKET_ENV`] until the host stops it. `name` is the
/// executable's, for its messages.
pub async fn main(name: &str, module: Arc<dyn Module>) -> std::process::ExitCode {
    use std::process::ExitCode;

    pcx_module_core::logs(name);
    let Some(socket) = std::env::var_os(SOCKET_ENV) else {
        eprintln!("{name}: a pcx module; the host starts it with {SOCKET_ENV} set");
        return ExitCode::from(2);
    };
    match serve(module, Path::new(&socket)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(module = name, error = %e, "the module stopped serving");
            ExitCode::FAILURE
        }
    }
}

/// Serve `module` on the Unix socket `socket` until the host calls Stop.
///
/// # Errors
/// The socket cannot be bound, or the server fails.
pub async fn serve(module: Arc<dyn Module>, socket: &Path) -> Result<(), ServeError> {
    let mut slots = tonic::service::Routes::builder();
    if let Some(provider) = module.model_provider() {
        slots.add_service(ModelService::new(provider).into_server());
    }
    if let Some(engine) = module.context_engine() {
        slots.add_service(ContextService::new(engine).into_server());
    }
    if let Some(turns) = module.turns() {
        slots.add_service(TurnsService::new(turns).into_server());
    }
    pcx_module_core::serve(Arc::new(AsProcess(module)), socket, slots).await
}

/// A module as a process: its start gets the handle over the callback
/// socket.
struct AsProcess(Arc<dyn Module>);

#[async_trait]
impl Process for AsProcess {
    fn manifest(&self) -> &Manifest {
        self.0.manifest()
    }

    async fn start(&self, start: Start) -> Result<(), ModuleError> {
        let host = Arc::new(RemoteHost::new(&start.host_socket));
        let channel = host.grpc();
        self.0
            .start(HostContext {
                host_api: start.host_api,
                config: start.config,
                host,
                channel,
            })
            .await
    }

    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        self.0.on_event(scope, event).await
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        self.0.resource(uri).await
    }

    async fn prompts(&self, scope: &Scope) -> Result<Vec<Prompt>, ModuleError> {
        self.0.prompts(scope).await
    }

    async fn prompt(
        &self,
        scope: &Scope,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        self.0.prompt(scope, name, arguments).await
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        self.0.stop().await
    }

    fn services(&self) -> Option<crate::grpc::GrpcChannel> {
        self.0.services()
    }
}
