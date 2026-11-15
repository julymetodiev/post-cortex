// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `ops` module: its manifest (its document kind, its configuration,
//! the modules it uses, the decisions it follows, its service) and the
//! decisions it hears of, an approved one's action run in the background,
//! and its look every minute at the actions that wait.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::SharedDocuments;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, DocumentKind, Event, EventKind, FailurePolicy, HostContext, Manifest,
    MemoryWrite, Module, ModuleError, ModuleErrorKind, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::module::{Effect, MemoryFilter, Rpc, Subscription};
use post_cortex_proto::ops::ops_server::OpsServer;
use post_cortex_proto::ops::{Action, OpsConfig};
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::adapters::{HostWorld, SystemClock};
use crate::domain::{ACTION, MODULE_ID};
use crate::handlers::Handlers;
use crate::service::{self, Service};

/// The modules it uses: the board (decisions, comments), memory (its
/// actions) and the channels (W29: Telegram, Discord).
const USES: [&str; 4] = ["board", "discord", "post-cortex.memory", "telegram"];

/// The board's decisions, whose writes it follows.
const DECISION: &str = "board/decision";

/// The module's service.
const SERVICE: &str = "pcx.ops.v1.Ops";

/// Each RPC and its tool name: all an agent may call (the policy or a
/// person approves each action).
const RPCS: [(&str, &str, Effect); 9] = [
    ("Restart", "ops_restart", Effect::External),
    ("DeletePod", "ops_delete_pod", Effect::External),
    ("CleanJobs", "ops_clean_jobs", Effect::External),
    ("Scale", "ops_scale", Effect::External),
    ("ResizeVolume", "ops_resize_volume", Effect::External),
    ("NewAllocation", "ops_new_allocation", Effect::External),
    ("RunPeriodic", "ops_run_periodic", Effect::External),
    ("RevertJobs", "ops_revert_jobs", Effect::External),
    ("ListActions", "ops_actions", Effect::Read),
];

/// The deadline of an approved action's run in the background.
const RUN: Duration = Duration::from_secs(120);

/// How long between two looks at the actions that wait.
const LOOK: Duration = Duration::from_secs(60);

/// How long until the waiting actions are asked for again, while memory
/// does not answer yet.
const AGAIN: Duration = Duration::from_secs(5);

/// What a started module works with.
pub struct Running {
    config: OpsConfig,
    world: HostWorld,
    channel: GrpcChannel,
    stop: CancellationToken,
}

impl Running {
    /// Its actions' documents in its workspace, the organisation's, for a
    /// call in `scope`: every caller reads them, and the module writes
    /// each (ARCHITECTURE section 8, operations).
    pub fn docs<'a>(&'a self, scope: &'a Scope) -> SharedDocuments<'a> {
        SharedDocuments::new(
            self.channel.clone(),
            scope,
            &self.config.workspace_id,
            vec![ACTION.to_string()],
        )
    }

    /// The service over `docs`, its actions' documents.
    pub fn service<'a>(&'a self, docs: &'a SharedDocuments<'a>) -> Service<'a> {
        Service {
            config: &self.config,
            docs,
            cluster: &self.world,
            nomad: &self.world,
            board: &self.world,
            teller: &self.world,
            clock: &SystemClock,
        }
    }

    fn scope(&self) -> Scope {
        Scope::own(
            MODULE_ID,
            &self.config.workspace_id,
            RUN,
            self.stop.child_token(),
        )
    }

    /// Until it stops, from when memory answers: a look every minute at
    /// the actions that still wait (one answered while the module was down
    /// is settled; one no one answers is said again, then given up).
    fn watch(self: &Arc<Self>) {
        let running = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let scope = running.scope();
                let docs = running.docs(&scope);
                let next = match running.service(&docs).look().await {
                    Ok(0) => LOOK,
                    Ok(n) => {
                        tracing::info!(actions = n, "waiting actions are settled");
                        LOOK
                    }
                    Err(e) => {
                        tracing::debug!("the waiting actions cannot be read yet: {e}");
                        AGAIN
                    }
                };
                tokio::select! {
                    () = running.stop.cancelled() => return,
                    () = tokio::time::sleep(next) => {}
                }
            }
        });
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
        .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "ops has not started"))
}

/// The `ops` module.
pub struct OpsModule {
    manifest: Manifest,
    running: Shared,
}

impl OpsModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Changes a running system only through typed actions (Kubernetes: restart, delete a pod, clean jobs, scale, grow a volume; Nomad: a new allocation, a periodic job launched, jobs reverted), each a board decision run once the team's trust or a person approves it.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::Network.into(), Capability::WriteMemory.into()],
                config_type: OpsConfig::full_name(),
                document_kinds: vec![DocumentKind {
                    name: "action".to_string(),
                    title_field: "what".to_string(),
                    searchable: None,
                    r#type: Action::full_name(),
                }],
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
                descriptors: post_cortex_proto::OPS_DESCRIPTOR_SET.to_vec(),
                rpcs: RPCS
                    .iter()
                    .map(|(method, tool, effect)| Rpc {
                        method: format!("/{SERVICE}/{method}"),
                        tool: (*tool).to_string(),
                        visibility: Vec::new(),
                        effect: (*effect).into(),
                    })
                    .collect(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for OpsModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for OpsModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config = service::checked(ctx.config()?)?;
        let stop = CancellationToken::new();
        let running = Arc::new(Running {
            world: HostWorld::new(ctx.host, ctx.channel.clone(), stop.clone()),
            channel: ctx.channel,
            config,
            stop,
        });
        running.watch();
        if let Some(old) = self.running.lock().replace(running) {
            old.stop.cancel();
        }
        Ok(())
    }

    /// Its `pcx.ops.v1.Ops` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(OpsServer::new(
            handlers,
        ))))
    }

    /// A decision written: its action runs in the background when it was
    /// approved; the event is answered at once.
    async fn on_event(&self, _scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        let running = running(&self.running)?;
        if let MemoryWrite::Document {
            id, deleted: false, ..
        } = MemoryWrite::from_event(event)?
        {
            tokio::spawn(async move {
                let scope = running.scope();
                let docs = running.docs(&scope);
                if let Err(e) = running.service(&docs).decided(&id).await {
                    tracing::warn!(decision = %id, error = %e, "an approved action did not run");
                }
            });
        }
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("ops serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(running) = self.running.lock().take() {
            running.stop.cancel();
        }
        Ok(())
    }
}
