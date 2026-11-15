// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `skills` module: its manifest (its document kind, its configuration,
//! the prompts it offers, the module it uses) and its tools and prompts
//! over the skills service, whose documents are its host handle's in the
//! library's workspace (its configuration's `workspace_id`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::SharedDocuments;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Prompt, PromptContent, Resource, Scope,
};
use post_cortex_proto::skills::SkillsConfig;
use post_cortex_proto::skills::skills_server::SkillsServer;
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::service::Service;
use crate::view;
use crate::{handlers, schemas, specs};

/// The memory module, whose tools keep the skills' documents.
const MEMORY: &str = "post-cortex.memory";

/// How long the module waits before it tries again to add the built-in
/// skills (the host makes memory callable once every module has started).
const AGAIN: Duration = Duration::from_secs(5);

/// The deadline of each of the module's own calls to memory.
const DEADLINE: Duration = Duration::from_secs(60);

/// What a started module works with.
#[derive(Clone)]
pub struct Running {
    /// Its channel to memory's documents (W23).
    channel: GrpcChannel,
    /// The library's workspace.
    workspace: String,
    /// Cancelled when it stops.
    stop: CancellationToken,
}

impl Running {
    /// The library's documents, the organisation's, for a call in `scope`:
    /// every caller reads the skills, and a skill is added or changed by a
    /// caller who reaches the library's workspace (ARCHITECTURE section 8,
    /// skills).
    pub fn docs<'a>(&'a self, scope: &'a Scope) -> SharedDocuments<'a> {
        SharedDocuments::new(self.channel.clone(), scope, &self.workspace, Vec::new())
    }

    /// The built-in skills added or renewed, once memory answers, in the
    /// module's own scope.
    fn seed(&self) {
        let running = self.clone();
        tokio::spawn(async move {
            loop {
                let wire = post_cortex_proto::module::Scope {
                    workspace_id: running.workspace.clone(),
                    author: MODULE_ID.to_string(),
                    ..Default::default()
                };
                let scope = Scope::from_wire(wire, running.stop.child_token(), DEADLINE);
                let docs = running.docs(&scope);
                match Service::new(&docs).seed().await {
                    Ok(written) => {
                        tracing::info!(
                            ?written,
                            "the built-in skills are in the library (added or renewed)"
                        );
                        return;
                    }
                    Err(e) => tracing::debug!("the built-in skills cannot be added yet: {e}"),
                }
                tokio::select! {
                    () = running.stop.cancelled() => return,
                    () = tokio::time::sleep(AGAIN) => {}
                }
            }
        });
    }
}

/// The running module, shared by the module and its service; none until
/// it starts.
pub type Shared = Arc<Mutex<Option<Running>>>;

/// The running module of `shared`.
///
/// # Errors
/// `FailedPrecondition` before it starts.
pub fn running(shared: &Shared) -> Result<Running, ModuleError> {
    shared.lock().clone().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            "skills has not started",
        )
    })
}

/// The `skills` module.
pub struct SkillsModule {
    manifest: Manifest,
    running: Shared,
}

impl SkillsModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "A library of skills (instructions) the workspace’s agents and people use; each is an MCP prompt.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                config_type: SkillsConfig::full_name(),
                document_kinds: schemas::document_kinds(),
                uses: vec![MEMORY.to_string()],
                prompts: true,
                views: view::views(),
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::SKILLS_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for SkillsModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for SkillsModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: SkillsConfig = ctx.config()?;
        if config.workspace_id.is_empty() {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "skills needs workspace_id, the library's workspace",
            )
            .with_field("workspace_id"));
        }
        let running = Running {
            channel: ctx.channel,
            workspace: config.workspace_id,
            stop: CancellationToken::new(),
        };
        running.seed();
        if let Some(old) = self.running.lock().replace(running) {
            old.stop.cancel();
        }
        Ok(())
    }

    /// Its `pcx.skills.v1.Skills` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(
            SkillsServer::new(handlers),
        )))
    }

    async fn prompts(&self, scope: &Scope) -> Result<Vec<Prompt>, ModuleError> {
        let running = running(&self.running)?;
        let docs = running.docs(scope);
        handlers::prompts(&Service::new(&docs)).await
    }

    async fn prompt(
        &self,
        scope: &Scope,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        let running = running(&self.running)?;
        let docs = running.docs(scope);
        handlers::prompt(&Service::new(&docs), name, arguments).await
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("skills serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(running) = self.running.lock().take() {
            running.stop.cancel();
        }
        Ok(())
    }
}
