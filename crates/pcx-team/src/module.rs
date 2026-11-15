// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `team` module: its manifest (document kinds, the modules it uses,
//! the board's changes it follows), its tools over the team service, whose
//! documents are its host handle's in the caller's workspace, and its
//! members' work in the background.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::agent::Turns;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, EventKind, FailurePolicy, HostContext, Manifest, MemoryFilter,
    MemoryWrite, Module, ModuleAvailable, ModuleError, ModuleErrorKind, ModuleHost, ModuleKind,
    Outcome, Resource, Scope, Subscription,
};
use post_cortex_proto::team::TeamConfig;
use post_cortex_proto::team::teams_server::TeamsServer;
use prost::Name;

use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::runner::Runner;
use crate::turns::TeamTurns;
use crate::{schemas, specs, view};

/// The modules the team uses: memory (its documents and sessions), the
/// board (its threads, comments and labels), the skills (their
/// instructions), the agent (its turns) and the tools its members' turns
/// reach (a module's turns reach only the modules it uses).
const USES: [&str; 14] = [
    "post-cortex.memory",
    "board",
    "skills",
    "agent",
    "tools-fs",
    "tools-shell",
    "tools-git",
    "observe",
    "ops",
    "scheduler",
    "workflow",
    "mail",
    "http",
    "dynamic",
];

/// The board's documents whose changes wake members.
const FOLLOWED: [&str; 2] = ["board/task", "board/comment"];

/// How long the host waits for the team to take a change (it reads it in
/// the background).
const EVENT_DEADLINE_MS: u32 = 2_000;

/// What a started module works with.
#[derive(Clone)]
pub struct Running {
    /// Its channel to the services of the modules it uses (W23).
    pub channel: GrpcChannel,
    /// Its host: the agent's turns.
    pub host: Arc<dyn ModuleHost>,
    /// Its members' work in the background.
    runner: Arc<Runner>,
}

impl Running {
    /// The turns of `workspace` that run now.
    pub fn busy(&self, workspace: &str) -> Vec<crate::progress::Busy> {
        self.runner.busy(workspace)
    }

    /// Stop `member`'s work on `task`; whether a turn of it was running.
    pub fn stop_work(&self, workspace: &str, task: &str, member: &str) -> bool {
        self.runner.stop_work(workspace, task, member)
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
        ModuleError::new(ModuleErrorKind::FailedPrecondition, "team has not started")
    })
}

/// The `team` module.
pub struct TeamModule {
    manifest: Manifest,
    running: Shared,
}

impl TeamModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Agents with roles and authority, teams of them, and the trust a person gives each team.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                document_kinds: schemas::document_kinds(),
                config_type: TeamConfig::full_name(),
                views: view::views(),
                uses: USES.iter().map(|m| (*m).to_string()).collect(),
                events: vec![Subscription {
                    kind: EventKind::MemoryWrite.into(),
                    deadline_ms: EVENT_DEADLINE_MS,
                    policy: FailurePolicy::FailOpen.into(),
                    memory: Some(MemoryFilter {
                        document_kinds: FOLLOWED.iter().map(|k| (*k).to_string()).collect(),
                        record_types: Vec::new(),
                        workspaces: Vec::new(),
                    }),
                },
                // A runner connected: the work waiting for it goes on.
                Subscription {
                    kind: EventKind::ModuleAvailable.into(),
                    deadline_ms: EVENT_DEADLINE_MS,
                    policy: FailurePolicy::FailOpen.into(),
                    memory: None,
                }],
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::TEAM_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                // A person talks with a member (W25).
                turns: true,
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for TeamModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for TeamModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: TeamConfig = ctx.config()?;
        let runner = Runner::new(Arc::clone(&ctx.host), ctx.channel.clone(), config);
        runner.recover();
        *self.running.lock() = Some(Running {
            runner,
            host: ctx.host,
            channel: ctx.channel,
        });
        Ok(())
    }

    /// Its `pcx.team.v1.Teams` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(TeamsServer::new(
            handlers,
        ))))
    }

    /// A change of the board is read in the background: the event is
    /// answered at once.
    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        let runner = running(&self.running)?.runner;
        // A runner connected: the work waiting for it goes on.
        if ModuleAvailable::from_event(event)?.is_some() {
            runner.resume();
            return Ok(Outcome::default());
        }
        match MemoryWrite::from_event(event)? {
            MemoryWrite::Document {
                workspace_id,
                id,
                kind,
                version,
                deleted: false,
            } => runner.changed(workspace_id, kind, id, version, scope.reach.clone()),
            // A deleted document or a record wakes no one; a feed that fell
            // behind lost changes no member may be woken for twice.
            MemoryWrite::Document { .. } | MemoryWrite::Record { .. } | MemoryWrite::FellBehind => {
            }
        }
        Ok(Outcome::default())
    }

    /// Its members' turns, as `RunTurn` asks them (`member`).
    fn turns(&self) -> Option<Arc<dyn Turns>> {
        Some(Arc::new(TeamTurns {
            shared: Arc::clone(&self.running),
        }))
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("team serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(running) = self.running.lock().take() {
            running.runner.stop();
        }
        Ok(())
    }
}
