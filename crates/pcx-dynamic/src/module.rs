// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `dynamic` module: its manifest, its settings read at start, the board's
//! approvals followed and read again.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::SharedDocuments;
use pcx_module_api::bus::Followed;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::dynamic::DynamicConfig;
use post_cortex_proto::dynamic::dynamic_server::DynamicServer;
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::adapters::{HostPorts, Launcher, LauncherConfig};
use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::service::{Ports, Service, Settings};
use crate::{specs, view};

/// The modules it uses.
const MEMORY: &str = "post-cortex.memory";
const BOARD: &str = "board";
const FILES: &str = "tools-fs";

/// Between reads when the host is not ready yet.
const AGAIN: Duration = Duration::from_secs(1);

/// The deadline of the module's own work.
const OWN_DEADLINE: Duration = Duration::from_secs(60);

/// The module at work.
pub struct Running {
    settings: Settings,
    ports: HostPorts,
    launcher: Launcher,
    stop: CancellationToken,
}

impl Running {
    /// Its documents in its workspace, the organisation's, for a call in
    /// `scope`: every caller reads the modules, and one is installed,
    /// edited or removed by a caller who reaches that workspace
    /// (docs/modules/dynamic.md, its store).
    pub fn docs<'a>(&'a self, scope: &'a Scope) -> SharedDocuments<'a> {
        SharedDocuments::new(
            self.ports.channel.clone(),
            scope,
            &self.settings.workspace,
            Vec::new(),
        )
    }

    /// The service over `docs`.
    pub fn service<'a>(&'a self, docs: &'a SharedDocuments<'a>) -> Service<'a> {
        Service::new(
            Ports {
                documents: docs,
                blobs: &self.ports,
                bundles: &self.launcher,
                board: &self.ports,
                reach: &self.ports,
                directory: &self.ports,
                events: &self.ports,
            },
            &self.settings,
        )
    }

    fn scope(&self) -> Scope {
        Scope::own(
            MODULE_ID,
            &self.settings.workspace,
            OWN_DEADLINE,
            self.stop.child_token(),
        )
    }

    /// Approvals made while it did not run read again, then the board followed.
    fn follow(self: &Arc<Self>) {
        let running = Arc::clone(self);
        tokio::spawn(async move {
            // Until it reads: the host may still be loading its modules.
            loop {
                let scope = running.scope();
                let docs = running.docs(&scope);
                match running.service(&docs).recheck().await {
                    Ok(()) => break,
                    Err(e) => {
                        tracing::debug!("the modules' decisions were not read again yet: {e}")
                    }
                }
                tokio::select! {
                    () = running.stop.cancelled() => return,
                    () = tokio::time::sleep(AGAIN) => {}
                }
            }
            loop {
                running.followed().await;
                tokio::select! {
                    () = running.stop.cancelled() => return,
                    () = tokio::time::sleep(AGAIN) => {}
                }
            }
        });
    }

    /// The bus's feed, until it ends or the module stops.
    async fn followed(&self) {
        let bus = match self.ports.host.bus().await {
            Ok(bus) => bus,
            Err(e) => {
                tracing::debug!("the bus cannot be followed yet: {e}");
                return;
            }
        };
        let mut feed = match bus.follow().await {
            Ok(feed) => feed,
            Err(e) => {
                tracing::debug!("the bus cannot be followed yet: {e}");
                return;
            }
        };
        loop {
            let next = tokio::select! {
                () = self.stop.cancelled() => return,
                next = feed.next() => next,
            };
            match next {
                Some(Ok(Followed::Event(event))) => {
                    if let Some(approved) = pcx_approval::heard(&event) {
                        let scope = self.scope();
                        let docs = self.docs(&scope);
                        if let Err(e) = self.service(&docs).decided(&approved).await {
                            tracing::warn!("an approval was not kept: {e}");
                            return;
                        }
                    }
                    if let Some(declined) = pcx_approval::declined(&event) {
                        let scope = self.scope();
                        let docs = self.docs(&scope);
                        if let Err(e) = self.service(&docs).declined(&declined).await {
                            tracing::warn!("a refusal was not kept: {e}");
                            return;
                        }
                    }
                    if let Err(e) = bus.ack(&event.name, event.position).await {
                        tracing::warn!(event = %event.name, "an event was not acknowledged: {e}");
                    }
                }
                Some(Ok(Followed::FellBehind(name))) => {
                    tracing::warn!(event = %name, "events were dropped before they were read");
                    let scope = self.scope();
                    let docs = self.docs(&scope);
                    if let Err(e) = self.service(&docs).recheck().await {
                        tracing::warn!("the modules' decisions were not read again: {e}");
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!("the bus's feed ended: {e}");
                    return;
                }
                None => return,
            }
        }
    }
}

/// The running module, shared by the module and its service; none until it
/// starts.
pub type Shared = Arc<Mutex<Option<Arc<Running>>>>;

/// The running module of `shared`.
///
/// # Errors
/// `Unavailable` before it starts.
pub fn running(shared: &Shared) -> Result<Arc<Running>, ModuleError> {
    shared.lock().clone().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::Unavailable,
            "the dynamic module has not started",
        )
    })
}

/// The module.
pub struct DynamicModule {
    manifest: Manifest,
    launcher: LauncherConfig,
    max_modules: Option<usize>,
    running: Shared,
}

impl DynamicModule {
    /// Its id.
    pub const ID: &'static str = MODULE_ID;

    /// With the most modules it keeps (the organisation's plan); none: no limit.
    #[must_use]
    pub fn with_max_modules(self, max_modules: Option<usize>) -> Self {
        Self {
            max_modules,
            ..self
        }
    }

    /// The module, reaching the launcher of `launcher`.
    #[must_use]
    pub fn new(launcher: LauncherConfig) -> Self {
        Self {
            max_modules: None,
            manifest: Manifest {
                description: "Modules in Python or TypeScript installed into a running host, each version checked in a locked run and approved by a person.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                config_type: DynamicConfig::full_name(),
                document_kinds: specs::document_kinds(),
                uses: vec![MEMORY.to_string(), BOARD.to_string(), FILES.to_string()],
                follows: vec![
                    pcx_approval::DECISION_APPROVED.to_string(),
                    pcx_approval::DECISION_DECLINED.to_string(),
                ],
                emits: specs::emits(),
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::DYNAMIC_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                views: view::views(),
                ..Manifest::default()
            },
            launcher,
            running: Shared::default(),
        }
    }
}

#[async_trait]
impl Module for DynamicModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: DynamicConfig = ctx.config()?;
        let running = Arc::new(Running {
            ports: HostPorts {
                host: ctx.host,
                channel: ctx.channel,
                workspace: config.workspace_id.clone(),
            },
            launcher: Launcher {
                config: self.launcher.clone(),
                owner: format!("check-{}", config.workspace_id),
            },
            settings: Settings {
                workspace: config.workspace_id,
                board: config.board,
                max_modules: self.max_modules,
            },
            stop: CancellationToken::new(),
        });
        running.follow();
        if let Some(old) = self.running.lock().replace(running) {
            old.stop.cancel();
        }
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri.to_string()))
    }

    fn services(&self) -> Option<GrpcChannel> {
        Some(grpc::routes(tonic::service::Routes::new(
            DynamicServer::new(Handlers {
                shared: Arc::clone(&self.running),
            }),
        )))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(running) = self.running.lock().take() {
            running.stop.cancel();
        }
        Ok(())
    }
}
