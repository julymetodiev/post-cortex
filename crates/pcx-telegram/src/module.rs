// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `telegram` module: its manifest (its configuration, the modules it
//! uses, the decisions it follows, the channel service it serves), its
//! poller of the bot's updates, and the decisions it hears of, each acted
//! on in the background.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, EventKind, FailurePolicy, HostContext, Manifest, MemoryWrite,
    Module, ModuleError, ModuleErrorKind, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::channel::channel_server::ChannelServer;
use post_cortex_proto::module::{MemoryFilter, Subscription};
use post_cortex_proto::telegram::TelegramConfig;
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::adapters::HostPorts;
use crate::bot::HttpBot;
use crate::domain::{API_URL, MODULE_ID};
use crate::handlers::Handlers;
use crate::prompts::Kept;
use crate::service::{self, Service};

/// The modules it uses: the board (decisions, approving them), the chief
/// (a person's questions) and memory (the decisions' documents).
const USES: [&str; 3] = ["board", "chief", "post-cortex.memory"];

/// The board's decisions, whose writes it follows.
const DECISION: &str = "board/decision";

/// The channel service it serves.
const SERVICE: &str = "pcx.channel.v1.Channel";

/// Seconds a long poll of the bot's updates waits.
const POLL: u32 = 30;

/// How long the poller waits after the Bot API failed.
const BACKOFF: Duration = Duration::from_secs(5);

/// What a started module works with.
pub struct Running {
    config: TelegramConfig,
    kept: Kept,
    bot: HttpBot,
    ports: HostPorts,
    stop: CancellationToken,
}

impl Running {
    /// The service over the module's ports.
    pub fn service(&self) -> Service<'_> {
        Service::new(
            &self.config,
            &self.kept,
            &self.bot,
            &self.ports,
            &self.ports,
        )
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
    shared
        .lock()
        .clone()
        .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "telegram has not started"))
}

/// The `telegram` module.
pub struct TelegramModule {
    manifest: Manifest,
    running: Shared,
}

impl TelegramModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Telegram: messages to its chats, decisions waiting for a person with buttons to approve them, and talking with the chief.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Channel.into(),
                capabilities: vec![Capability::Network.into()],
                config_type: TelegramConfig::full_name(),
                uses: USES.iter().map(|m| (*m).to_string()).collect(),
                events: vec![Subscription {
                    kind: EventKind::MemoryWrite.into(),
                    deadline_ms: 5_000,
                    policy: FailurePolicy::FailOpen.into(),
                    memory: Some(MemoryFilter {
                        document_kinds: vec![DECISION.to_string()],
                        record_types: Vec::new(),
                        workspaces: Vec::new(),
                    }),
                }],
                services: vec![SERVICE.to_string()],
                descriptors: post_cortex_proto::TELEGRAM_DESCRIPTOR_SET.to_vec(),
                rpcs: vec![post_cortex_proto::module::Rpc {
                    method: format!("/{SERVICE}/Send"),
                    tool: "channel_send".to_string(),
                    visibility: Vec::new(),
                    effect: post_cortex_proto::module::Effect::External.into(),
                }],
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for TelegramModule {
    fn default() -> Self {
        Self::new()
    }
}

/// Read the bot's updates until `running` stops.
fn poll(running: Arc<Running>) {
    tokio::spawn(async move {
        loop {
            let service = running.service();
            let polled = tokio::select! {
                () = running.stop.cancelled() => break,
                polled = service.poll(POLL) => polled,
            };
            if let Err(e) = polled {
                tracing::warn!(error = %e, "the Telegram updates could not be read");
                tokio::select! {
                    () = running.stop.cancelled() => break,
                    () = tokio::time::sleep(BACKOFF) => {}
                }
            }
        }
    });
}

#[async_trait]
impl Module for TelegramModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message; the
    /// module checks what the message cannot say, then reads its bot.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config = service::checked(ctx.config()?)?;
        let api = if config.api_url.trim().is_empty() {
            API_URL.to_string()
        } else {
            config.api_url.clone()
        };
        let stop = CancellationToken::new();
        let running = Arc::new(Running {
            bot: HttpBot::new(&api, &config.bot_token)?,
            ports: HostPorts::new(ctx.channel, stop.clone()),
            kept: Kept::default(),
            config,
            stop,
        });
        poll(Arc::clone(&running));
        if let Some(old) = self.running.lock().replace(running) {
            old.stop.cancel();
        }
        Ok(())
    }

    /// Its `pcx.channel.v1.Channel` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(
            ChannelServer::new(handlers),
        )))
    }

    /// A decision written is acted on in the background: the event is
    /// answered at once.
    async fn on_event(&self, _scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        let running = running(&self.running)?;
        if let MemoryWrite::Document {
            workspace_id,
            id,
            deleted: false,
            ..
        } = MemoryWrite::from_event(event)?
        {
            tokio::spawn(async move {
                if let Err(e) = running.service().decision_written(&workspace_id, &id).await {
                    tracing::warn!(decision = %id, error = %e, "a decision was not sent to Telegram");
                }
            });
        }
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("telegram serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(running) = self.running.lock().take() {
            running.stop.cancel();
        }
        Ok(())
    }
}
