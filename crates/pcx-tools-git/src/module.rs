// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `tools-git` module: its manifest, its configuration (`path`: the
//! `PATH` of git; `author_name` and `author_email`: its commits' author;
//! `push`: where and what it pushes) and
//! its tools over the sandbox its host binds to the policy the operator
//! grants it.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::sandbox::Sandbox;
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleHost, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::tools_git::GitConfig;
use post_cortex_proto::tools_git::git_server::GitServer;
use prost::Name;

use crate::git::{Author, Git};
use crate::handlers::Handlers;
use crate::push::Push;
use crate::specs;

/// What a started module holds: its host handle, git's `PATH` and its
/// commits' author.
pub struct Started {
    /// Its host handle (its sandbox).
    pub host: Arc<dyn ModuleHost>,
    path: Option<String>,
    author: Option<Author>,
    push: Option<Push>,
}

impl Started {
    /// Git in `sandbox`, with the module's `PATH`, author and push.
    pub fn git<'a>(&'a self, sandbox: &'a dyn Sandbox) -> Git<'a> {
        Git::new(
            sandbox,
            self.path.as_deref(),
            self.author.as_ref(),
            self.push.as_ref(),
        )
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
            "tools-git has not started",
        )
    })
}

/// The `tools-git` module.
pub struct GitModule {
    manifest: Manifest,
    started: Shared,
}

impl GitModule {
    /// The module's id.
    pub const ID: &'static str = "tools-git";

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description:
                    "Git in the repositories under its roots: status, diff, branches, commits and, when configured, pushing a task's branch."
                        .to_string(),
                id: Self::ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::Filesystem.into(), Capability::Spawn.into()],
                config_type: GitConfig::full_name(),
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::TOOLS_GIT_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            started: Shared::default(),
        }
    }
}

impl Default for GitModule {
    fn default() -> Self {
        Self::new()
    }
}

/// The author `config` names: both its name and its email, or neither.
fn author(config: &GitConfig) -> Result<Option<Author>, ModuleError> {
    match (config.author_name.as_str(), config.author_email.as_str()) {
        ("", "") => Ok(None),
        (name, email) if !name.is_empty() && !email.is_empty() => Ok(Some(Author {
            name: name.to_string(),
            email: email.to_string(),
        })),
        _ => Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            "tools-git's configuration names both author_name and author_email, or neither",
        )),
    }
}

#[async_trait]
impl Module for GitModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: GitConfig = ctx.config()?;
        let author = author(&config)?;
        let push = Push::of(config.push)?;
        *self.started.lock() = Some(Arc::new(Started {
            host: ctx.host,
            path: Some(config.path).filter(|p| !p.is_empty()),
            author,
            push,
        }));
        Ok(())
    }

    /// Its `pcx.tools.git.v1.Git` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.started),
        };
        Some(grpc::routes(tonic::service::Routes::new(GitServer::new(
            handlers,
        ))))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("tools-git serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        *self.started.lock() = None;
        Ok(())
    }
}
