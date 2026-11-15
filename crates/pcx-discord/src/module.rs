// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `discord` module: its manifest (its configuration, the services it
//! serves, the tools and the events it names), its check of the bot's
//! rights when it starts, and its listener on the Gateway.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleHost, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::channel::channel_server::ChannelServer;
use post_cortex_proto::discord::discord_server::DiscordServer;
use post_cortex_proto::discord::{DiscordConfig, ServerStatus};
use post_cortex_proto::module::{Effect, EmittedEvent, Rpc};
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::adapters::HostEvents;
use crate::adapters::rest::HttpRest;
use crate::adapters::socket::WebSocketGateway;
use crate::domain::gateway::Session;
use crate::domain::heard::{AUTOMOD_ACTED, MEMBER_JOINED, MESSAGE_CREATED, POST_CREATED};
use crate::domain::rights::Server;
use crate::domain::{API_URL, MODULE_ID};
use crate::handlers::Handlers;
use crate::service::{self, Kept, Linked, Service};

/// The channel service it serves beside Telegram.
const CHANNEL: &str = "pcx.channel.v1.Channel";

/// Its own service.
const SERVICE: &str = "pcx.discord.v1.Discord";

/// How long the listener waits before it connects again: Discord asks a
/// bot not to connect in a loop.
const BACKOFF: Duration = Duration::from_secs(5);

/// How long it waits before it reads its server again, when the bot lacks
/// something there: a person who adds the bot again saves nothing anew.
const LOOK_AGAIN: Duration = Duration::from_secs(60);

/// What a started module works with.
pub struct Running {
    config: DiscordConfig,
    linked: Linked,
    kept: Kept,
    rest: HttpRest,
    events: HostEvents,
    /// The module's handle on its host, which it tells what stops it.
    host: Arc<dyn ModuleHost>,
    stop: CancellationToken,
}

impl Running {
    /// The service over the module's ports, in the server as it was read.
    ///
    /// # Errors
    /// Why the server was not read ([`Linked::server`]).
    pub fn service(&self) -> Result<Service<'_>, ModuleError> {
        Ok(self.over(self.linked.server()?))
    }

    /// Whether the module works with its server, and why not.
    pub fn status(&self) -> ServerStatus {
        self.linked.status()
    }

    fn over(&self, server: Arc<Server>) -> Service<'_> {
        Service::new(&self.config, server, &self.kept, &self.rest, &self.events)
    }

    /// Tell the host what stops the module from working in its server
    /// (`problem`; empty: nothing does), for its health and its page in the
    /// app (docs/host.md, a module's problem).
    async fn report(&self, problem: &str) {
        if let Err(e) = self.host.report(problem).await {
            tracing::warn!(error = %e, "the host was not told how the module fares");
        }
    }

    /// Read the server unless it was read, then listen on one connection
    /// of the Gateway, going on with `session`.
    async fn connected(&self, session: &mut Session) -> Result<(), ModuleError> {
        let server = self.linked.read(&self.config, &self.rest).await?;
        self.report("").await;
        self.over(server)
            .listen(&WebSocketGateway, session, &self.stop)
            .await
    }
}

/// The running module, shared by the module and its services; none until
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
        .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "discord has not started"))
}

/// The `discord` module.
pub struct DiscordModule {
    manifest: Manifest,
    running: Shared,
}

/// Its RPCs: the channel's `Send` no tool (an RPC's path names one tool,
/// and Telegram's is `channel_send`; docs/host.md), its own each a tool.
fn rpcs() -> Vec<Rpc> {
    [
        (CHANNEL, "Send", ""),
        (SERVICE, "Post", "discord_post"),
        (SERVICE, "Reply", "discord_reply"),
        (SERVICE, "GiveRole", "discord_give_role"),
        (SERVICE, "TagPost", "discord_tag_post"),
        (SERVICE, "Timeout", "discord_timeout"),
        (SERVICE, "LockThread", "discord_lock_thread"),
        (SERVICE, "Ban", "discord_ban"),
    ]
    .map(|(service, method, tool)| Rpc {
        method: format!("/{service}/{method}"),
        tool: tool.to_string(),
        visibility: Vec::new(),
        effect: Effect::External.into(),
    })
    .into_iter()
    .chain([Rpc {
        method: format!("/{SERVICE}/Status"),
        tool: "discord_status".to_string(),
        visibility: Vec::new(),
        effect: Effect::Read.into(),
    }])
    .collect()
}

/// The events it emits (docs/modules/discord.md, what it hears).
fn events() -> Vec<EmittedEvent> {
    [
        (
            POST_CREATED,
            "PostCreated",
            "A person opened a post in a forum the bot hears.",
        ),
        (
            MESSAGE_CREATED,
            "MessageCreated",
            "A person wrote in a channel the bot hears, or in a post of one.",
        ),
        (MEMBER_JOINED, "MemberJoined", "A person joined the server."),
        (
            AUTOMOD_ACTED,
            "AutomodActed",
            "The server's AutoMod acted on what a member wrote.",
        ),
    ]
    .map(|(name, message, description)| EmittedEvent {
        name: name.to_string(),
        r#type: format!("pcx.discord.v1.{message}"),
        description: description.to_string(),
    })
    .to_vec()
}

impl DiscordModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Discord: messages to a server's channels, what its people write as events for workflows, posts, replies, roles, forum tags and moderation.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Channel.into(),
                capabilities: vec![Capability::Network.into()],
                config_type: DiscordConfig::full_name(),
                services: vec![CHANNEL.to_string(), SERVICE.to_string()],
                descriptors: post_cortex_proto::DISCORD_DESCRIPTOR_SET.to_vec(),
                rpcs: rpcs(),
                emits: events(),
                workflow_templates: crate::templates::templates(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for DiscordModule {
    fn default() -> Self {
        Self::new()
    }
}

/// Listen on the Gateway until `running` stops, a connection after
/// another, one session through them; the server is read first, again
/// after a pause for as long as the bot lacks something there.
fn listen(running: Arc<Running>) {
    tokio::spawn(async move {
        let mut session = Session::default();
        loop {
            let listened = running.connected(&mut session).await;
            if running.stop.is_cancelled() {
                break;
            }
            let pause = match listened {
                Err(e) if e.kind == ModuleErrorKind::FailedPrecondition => {
                    tracing::warn!(error = %e, "the Discord bot cannot work in its server");
                    running.report(&e.message).await;
                    LOOK_AGAIN
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Discord could not be listened to");
                    BACKOFF
                }
                Ok(()) => BACKOFF,
            };
            tokio::select! {
                () = running.stop.cancelled() => break,
                () = tokio::time::sleep(pause) => {}
            }
        }
    });
}

#[async_trait]
impl Module for DiscordModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message; the
    /// module checks what the message cannot say, then listens. What its
    /// bot may do in the server is read by the listener, not here: a host
    /// starts whatever Discord answers (docs/modules/discord.md, connecting
    /// a server).
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config = service::checked(ctx.config()?)?;
        let rest = HttpRest::new(API_URL, &config.bot_token)?;
        let stop = CancellationToken::new();
        let running = Arc::new(Running {
            events: HostEvents::new(Arc::clone(&ctx.host), &config.workspace_id, stop.clone()),
            host: ctx.host,
            kept: Kept::default(),
            linked: Linked::default(),
            config,
            rest,
            stop,
        });
        listen(Arc::clone(&running));
        if let Some(old) = self.running.lock().replace(running) {
            old.stop.cancel();
        }
        Ok(())
    }

    /// Its `pcx.channel.v1.Channel` and `pcx.discord.v1.Discord` services.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(
            tonic::service::Routes::new(ChannelServer::new(handlers.clone()))
                .add_service(DiscordServer::new(handlers)),
        ))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("discord serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(running) = self.running.lock().take() {
            running.stop.cancel();
        }
        Ok(())
    }
}
