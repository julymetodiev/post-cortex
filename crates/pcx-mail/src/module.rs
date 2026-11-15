// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `mail` module: its manifest (its log's kind, its configuration,
//! the modules it uses, its service), its settings read at start with the
//! transport they name, and the daily pruning of its log.

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
use post_cortex_proto::mail::MailConfig;
use post_cortex_proto::mail::mail_server::MailServer;
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::adapters::{self, SystemClock};
use crate::domain::{self, MODULE_ID, Settings};
use crate::handlers::Handlers;
use crate::ports::Transport;
use crate::service::Service;
use crate::specs;

/// The memory module, which keeps the log.
const MEMORY: &str = "post-cortex.memory";

/// The deadline of a pruning's calls to memory.
const PRUNE_DEADLINE: Duration = Duration::from_secs(600);

/// The module at work: its settings, its transport, the module's channel,
/// and the gate its sends pass one at a time.
pub struct Running {
    /// The settings, read and checked.
    pub settings: Settings,
    /// The transport they name.
    pub transport: Arc<dyn Transport>,
    /// The module's channel to memory.
    pub channel: GrpcChannel,
    /// Held while a send is counted, sent and logged.
    pub gate: tokio::sync::Mutex<()>,
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

    /// The service over `docs`, with the module's settings, transport and
    /// gate.
    pub fn service<'a>(&'a self, docs: &'a SharedDocuments<'a>) -> Service<'a> {
        Service::new(
            docs,
            &SystemClock,
            self.transport.as_ref(),
            &self.settings,
            &self.gate,
        )
    }

    /// Its documents in its workspace, the organisation's, for a call in
    /// `scope`: every caller reads the templates and the log, the module
    /// writes a send's record, and a template is changed by a caller who
    /// reaches that workspace (docs/modules/mail.md, its store).
    pub fn docs<'a>(&'a self, scope: &'a Scope) -> SharedDocuments<'a> {
        SharedDocuments::new(
            self.channel.clone(),
            scope,
            &self.settings.workspace,
            vec![domain::sent_kind()],
        )
    }

    async fn prune(&self) {
        let scope = self.scope();
        let docs = self.docs(&scope);
        match self.service(&docs).prune().await {
            Ok(removed) => tracing::info!(removed, "the mail log was pruned"),
            Err(e) => tracing::warn!(error = %e, "the mail log could not be pruned"),
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
            "the mail module has not started",
        )
    })
}

/// The `mail` module.
pub struct MailModule {
    manifest: Manifest,
    running: Shared,
}

impl MailModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Mail with the organisation’s own credentials (SMTP, Resend or Postmark), each send kept in a log.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into(), Capability::Network.into()],
                config_type: MailConfig::full_name(),
                document_kinds: specs::document_kinds(),
                uses: vec![MEMORY.to_string()],
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::MAIL_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for MailModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for MailModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message; the
    /// module checks what the message cannot say.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: MailConfig = ctx.config()?;
        let settings = domain::settings(config).map_err(|rule| {
            let field = rule.field.clone();
            ModuleError::new(ModuleErrorKind::InvalidArgument, rule.to_string()).with_field(field)
        })?;
        let transport = adapters::transport(&settings.route)?;
        let running = Arc::new(Running {
            settings,
            transport,
            channel: ctx.channel,
            gate: tokio::sync::Mutex::new(()),
            stop: CancellationToken::new(),
        });
        running.prune_daily();
        if let Some(old) = self.running.lock().replace(running) {
            old.stop.cancel();
        }
        Ok(())
    }

    /// Its `pcx.mail.v1.Mail` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(MailServer::new(
            handlers,
        ))))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("the mail module serves no resource {uri}"),
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
