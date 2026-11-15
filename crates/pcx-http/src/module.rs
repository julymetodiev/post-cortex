// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `http` module: its manifest (its kinds, its configuration, the
//! modules it uses, its service), its settings read at start with the
//! built-in presets, and the daily pruning of its log.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::SharedDocuments;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::http::http_server::HttpServer;
use post_cortex_proto::http::{HttpConfig, Preset};
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::adapters::{self, Reqwest, SystemClock, SystemResolver};
use crate::domain::{self, MODULE_ID, Settings};
use crate::handlers::Handlers;
use crate::service::{Gates, Parts, Service};
use crate::specs;

/// The memory module, which keeps the presets and the log.
const MEMORY: &str = "post-cortex.memory";

/// The deadline of a pruning's calls to memory.
const PRUNE_DEADLINE: Duration = Duration::from_secs(600);

/// The module at work: its settings, the built-in presets, the module's
/// channel, the day's count and the requests in flight.
pub struct Running {
    /// The settings, read and checked.
    pub settings: Settings,
    /// The built-in presets.
    pub built_in: Vec<Preset>,
    /// The module's channel to memory.
    pub channel: GrpcChannel,
    /// The day's count and the requests in flight.
    pub gates: Gates,
    stop: CancellationToken,
}

impl Running {
    /// The scope of the module's own calls (its pruning).
    fn scope(&self) -> Scope {
        Scope::own(
            MODULE_ID,
            &self.settings.workspace,
            PRUNE_DEADLINE,
            self.stop.child_token(),
        )
    }

    /// Prune the log once a day (`pcx_module_api::daily`), until the
    /// module stops.
    fn prune_daily(self: &Arc<Self>) {
        let running = Arc::clone(self);
        pcx_module_api::daily(self.stop.child_token(), move || {
            let running = Arc::clone(&running);
            async move { running.prune().await }
        });
    }

    /// The service over `docs`, with the module's settings, presets and
    /// gates.
    pub fn service<'a>(&'a self, docs: &'a SharedDocuments<'a>) -> Service<'a> {
        Service::new(Parts {
            docs,
            clock: &SystemClock,
            resolver: &SystemResolver,
            client: &Reqwest,
            settings: &self.settings,
            built_in: &self.built_in,
            gates: &self.gates,
        })
    }

    /// Its documents in its workspace, the organisation's, for a call in
    /// `scope`: every caller reads the presets and the log, the module
    /// writes a call's record, and a preset is changed by a caller who
    /// reaches that workspace (docs/modules/http.md, its store).
    pub fn docs<'a>(&'a self, scope: &'a Scope) -> SharedDocuments<'a> {
        SharedDocuments::new(
            self.channel.clone(),
            scope,
            &self.settings.workspace,
            vec![domain::call_kind()],
        )
    }

    async fn prune(&self) {
        let scope = self.scope();
        let docs = self.docs(&scope);
        match self.service(&docs).prune().await {
            Ok(removed) => tracing::info!(removed, "the http log was pruned"),
            Err(e) => tracing::warn!(error = %e, "the http log could not be pruned"),
        }
    }
}

/// The running module, shared by the module and its service; none until
/// it starts.
pub type Shared = Arc<Mutex<Option<Arc<Running>>>>;

/// The running module of `shared`.
///
/// # Errors
/// `Unavailable` before it starts.
pub fn running(shared: &Shared) -> Result<Arc<Running>, ModuleError> {
    shared.lock().clone().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::Unavailable,
            "the http module has not started",
        )
    })
}

/// The `http` module.
pub struct HttpModule {
    manifest: Manifest,
    public_only: bool,
    running: Shared,
}

impl HttpModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The argument of its process that refuses private networks (the
    /// cloud's catalog passes it).
    pub const PUBLIC_ONLY: &'static str = "--public-only";

    /// The module; `public_only`: it refuses a configuration that reaches
    /// private networks.
    #[must_use]
    pub fn new(public_only: bool) -> Self {
        Self {
            manifest: Manifest {
                description: "Calls of outside HTTP APIs through connections that keep the organisation’s credentials, each call kept in a log.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into(), Capability::Network.into()],
                config_type: HttpConfig::full_name(),
                document_kinds: specs::document_kinds(),
                uses: vec![MEMORY.to_string()],
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::HTTP_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            public_only,
            running: Shared::default(),
        }
    }
}

#[async_trait]
impl Module for HttpModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message; the
    /// module checks what the message cannot say.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: HttpConfig = ctx.config()?;
        let settings = domain::settings(config, self.public_only).map_err(|rule| {
            let field = rule.field.clone();
            ModuleError::new(rule.kind, rule.to_string()).with_field(field)
        })?;
        let running = Arc::new(Running {
            settings,
            built_in: adapters::built_in()?,
            channel: ctx.channel,
            gates: Gates::default(),
            stop: CancellationToken::new(),
        });
        running.prune_daily();
        if let Some(old) = self.running.lock().replace(running) {
            old.stop.cancel();
        }
        Ok(())
    }

    /// Its `pcx.http.v1.Http` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(HttpServer::new(
            handlers,
        ))))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("the http module serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(running) = self.running.lock().take() {
            running.stop.cancel();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
