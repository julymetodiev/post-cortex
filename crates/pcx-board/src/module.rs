// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `board` module: its manifest (document kinds, view, the modules it
//! uses) and its tools over the board service, whose ports are its host
//! handle in the caller's scope and workspace.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleHost, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::board::boards_server::BoardsServer;
use post_cortex_proto::module::EmittedEvent;

use crate::adapters::{
    DECISION_APPROVED, DECISION_DECLINED, DECISION_MADE, TASK_CREATED, TASK_MOVED,
};
use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::{schemas, specs, view};

/// The memory module, whose tools keep the board's documents and records.
const MEMORY: &str = "post-cortex.memory";

/// The git tools, which give a branch's commits.
const GIT: &str = "tools-git";

/// What a started module works with.
#[derive(Clone)]
pub struct Running {
    /// Its channel to the services of the modules it uses (W23).
    pub channel: GrpcChannel,
    /// Its handle on the host: the bus its events go to.
    pub host: Arc<dyn ModuleHost>,
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
        ModuleError::new(ModuleErrorKind::FailedPrecondition, "board has not started")
    })
}

/// The `board` module.
pub struct BoardModule {
    manifest: Manifest,
    running: Shared,
}

impl BoardModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Boards, tasks, plans, comments and decisions; decisions with a risk wait for a person.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into()],
                document_kinds: schemas::document_kinds(),
                views: view::views(),
                uses: vec![MEMORY.to_string(), GIT.to_string()],
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::BOARD_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                emits: events(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

/// The events it emits (docs/modules/board.md, events).
fn events() -> Vec<EmittedEvent> {
    [
        (TASK_CREATED, "TaskCreated", "A task was added."),
        (TASK_MOVED, "TaskMoved", "A task changed column."),
        (
            DECISION_MADE,
            "DecisionMade",
            "A decision was taken on a task.",
        ),
        (
            DECISION_APPROVED,
            "DecisionApproved",
            "A decision was approved, by a person or the team's policy.",
        ),
        (
            DECISION_DECLINED,
            "DecisionDeclined",
            "A person declined a decision that waited.",
        ),
    ]
    .map(|(name, message, description)| EmittedEvent {
        name: name.to_string(),
        r#type: format!("pcx.board.v1.{message}"),
        description: description.to_string(),
    })
    .to_vec()
}

impl Default for BoardModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for BoardModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.running.lock() = Some(Running {
            channel: ctx.channel,
            host: ctx.host,
        });
        Ok(())
    }

    /// Its `pcx.board.v1.Boards` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(
            BoardsServer::new(handlers),
        )))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("board serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        *self.running.lock() = None;
        Ok(())
    }
}
