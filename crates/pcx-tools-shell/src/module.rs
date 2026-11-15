// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `tools-shell` module: its manifest, its configuration (`path`: the
//! `PATH` of its processes; `env`: their other variables) and its tools
//! over the sandbox its host binds to the policy the operator grants it.
//! Its stop ends its tmux server.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::AgentAuthor;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::sandbox::Sandbox;
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleHost, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::tools_shell::ShellConfig;
use post_cortex_proto::tools_shell::shell_server::ShellServer;
use prost::Name;

use crate::handlers::Handlers;
use crate::shell::Shell;
use crate::specs;
use crate::tmux::Tmux;

/// What a started module holds: its host handle and configuration.
pub struct Started {
    /// Its host handle (its sandbox).
    pub host: Arc<dyn ModuleHost>,
    /// `PATH` of its processes; none: the module API's default.
    path: Option<String>,
    /// Their other variables.
    env: Vec<(String, String)>,
}

impl Started {
    /// The processes of `sandbox`, with the module's `PATH` and variables,
    /// for a call by `agent` (none: a person's or the module's own).
    pub fn shell<'a>(&'a self, sandbox: &'a dyn Sandbox, agent: Option<&AgentAuthor>) -> Shell<'a> {
        Shell::new(sandbox, self.path.as_deref(), &self.env, agent)
    }
}

/// The started module, shared by the module and its service; none until
/// it starts.
pub type Shared = Arc<Mutex<Option<Arc<Started>>>>;

/// The started module of `shared`.
///
/// # Errors
/// `FailedPrecondition` before it starts.
pub fn started(shared: &Shared) -> Result<Arc<Started>, ModuleError> {
    shared.lock().clone().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            "tools-shell has not started",
        )
    })
}

/// The `tools-shell` module.
pub struct ShellModule {
    manifest: Manifest,
    started: Shared,
}

impl ShellModule {
    /// The module's id.
    pub const ID: &'static str = "tools-shell";

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Runs commands and long-running tmux sessions in its sandbox."
                    .to_string(),
                id: Self::ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::Filesystem.into(), Capability::Spawn.into()],
                config_type: ShellConfig::full_name(),
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::TOOLS_SHELL_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            started: Shared::default(),
        }
    }
}

impl Default for ShellModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ShellModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: ShellConfig = ctx.config()?;
        let env = variables(config.env)?;
        *self.started.lock() = Some(Arc::new(Started {
            host: ctx.host,
            path: Some(config.path).filter(|p| !p.is_empty()),
            env,
        }));
        Ok(())
    }

    /// Its `pcx.tools.shell.v1.Shell` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.started),
        };
        Some(grpc::routes(tonic::service::Routes::new(ShellServer::new(
            handlers,
        ))))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("tools-shell serves no resource {uri}"),
        ))
    }

    /// End the tmux server, so no session outlives the module.
    async fn stop(&self) -> Result<(), ModuleError> {
        let Some(started) = self.started.lock().take() else {
            return Ok(());
        };
        let sandbox = started.host.sandbox().await?;
        Tmux::new(&started.shell(sandbox.as_ref(), None))
            .stop()
            .await
    }
}

/// The variables of the configuration, as a process gets them.
///
/// # Errors
/// `InvalidArgument` for a variable without a name, or one the module or
/// the sandbox sets.
fn variables(
    env: Vec<post_cortex_proto::tools_shell::Variable>,
) -> Result<Vec<(String, String)>, ModuleError> {
    env.into_iter()
        .map(|v| {
            let name = v.name.trim().to_string();
            if name.is_empty() || name.contains('=') {
                return Err(ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    format!("env: {:?} is no variable name", v.name),
                )
                .with_field("env"));
            }
            if pcx_module_api::sandbox::OWN_VARIABLES.contains(&name.as_str()) {
                return Err(ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    format!("env: {name} is set by the module (path) or the sandbox, not here"),
                )
                .with_field("env"));
            }
            Ok((name, v.value))
        })
        .collect()
}
