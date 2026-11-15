// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `workflow` module: its manifest (its kinds, events, configuration,
//! service; it uses every module and follows their events), its one engine
//! built at start over its ports in each workspace, the runs left open
//! driven again, and the bus followed for the runs that wait.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::Documents;
use pcx_module_api::bus::Followed;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, EVERY_MODULE, Event, HostContext, Manifest, Module, ModuleError,
    ModuleErrorKind, ModuleHost, ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::workflow::WorkflowConfig;
use post_cortex_proto::workflow::workflows_server::WorkflowsServer;
use prost::Name;
use tokio_util::sync::CancellationToken;

use crate::adapters::{
    DocumentRuns, DocumentWorkflows, DryStores, Host, OwnDocuments, SystemClock,
};
use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::ports::Events;
use crate::service::Service;
use crate::service::engine::{Engine, Ports, Settings, Spaces};
use crate::specs;

/// How long the module waits before it tries again to reach memory (the
/// host makes it callable once every module has started) or the bus.
const AGAIN: Duration = Duration::from_secs(5);

/// The deadline of the module's own look at the host's workspaces.
const LOOK: Duration = Duration::from_secs(60);

/// The engine's ports in each workspace, over the host.
struct HostSpaces {
    host: Arc<dyn ModuleHost>,
    channel: GrpcChannel,
    /// The workspace the settings name.
    workspace: String,
    /// The board the settings name, for that workspace's approvals.
    board: String,
}

impl HostSpaces {
    fn host(&self, workspace: &str) -> Host {
        Host::new(
            Arc::clone(&self.host),
            self.channel.clone(),
            workspace.to_string(),
        )
    }
}

impl Spaces for HostSpaces {
    fn of(&self, workspace: &str) -> (Ports, Settings) {
        let host = self.host(workspace);
        let docs: Arc<dyn Documents> = Arc::new(OwnDocuments::new(
            self.channel.clone(),
            workspace.to_string(),
        ));
        let runs = Arc::new(DocumentRuns::new(Arc::clone(&docs)));
        let workflows: Arc<DocumentWorkflows> = Arc::new(DocumentWorkflows::new(docs));
        let dry = DryStores {
            workflows: workflows.clone(),
            state: runs.clone(),
            blobs: Arc::new(host.clone()),
        };
        let ports = Ports {
            workflows,
            runs: runs.clone(),
            state: runs,
            tools: Arc::new(host.clone()),
            models: Arc::new(host.clone()),
            turns: Arc::new(host.clone()),
            events: Arc::new(host.clone()),
            blobs: Arc::new(host),
            clock: Arc::new(SystemClock),
            dry: Arc::new(dry),
        };
        let settings = Settings {
            workspace: workspace.to_string(),
            board: self.board.clone(),
            own_board: workspace != self.workspace,
            dry: None,
        };
        (ports, settings)
    }
}

/// The module at work for one call: the engine in the workspace the call
/// is asked from, and that workspace's workflows.
pub struct Running {
    /// The engine, in the call's workspace.
    pub engine: Arc<Engine>,
    /// Its ports over the host, in that workspace.
    host: Host,
    channel: GrpcChannel,
}

impl Running {
    /// The service of the workflows, for a call in `scope` (its writes are
    /// its author's).
    pub fn workflows(&self, scope: &Scope) -> DocumentWorkflows {
        let docs: Arc<dyn Documents> = Arc::new(OwnDocuments::of_call(
            self.channel.clone(),
            self.engine.workspace().to_string(),
            scope,
        ));
        DocumentWorkflows::new(docs)
    }

    /// The service over `workflows` and the host's tools.
    pub fn service<'a>(&'a self, workflows: &'a DocumentWorkflows) -> Service<'a> {
        Service::new(workflows, &self.host)
    }
}

/// The module at work: one engine for every workspace's workflows
/// (docs/modules/workflow.md, its workspaces).
pub struct Started {
    /// The engine, in the workspace the settings name.
    engine: Arc<Engine>,
    spaces: Arc<HostSpaces>,
    stop: CancellationToken,
}

impl Started {
    /// The scope of the module's own look at the host's workspaces.
    fn scope(&self) -> Scope {
        Scope::own(
            MODULE_ID,
            &self.spaces.workspace,
            LOOK,
            self.stop.child_token(),
        )
    }

    /// The module at work in the workspace a call in `scope` is asked
    /// from; asked from none, in the one the settings name.
    ///
    /// # Errors
    /// `Denied` for a workspace outside the call's reach.
    pub fn asked_from(&self, scope: &Scope) -> Result<Running, ModuleError> {
        let workspace = scope
            .workspace_id
            .as_deref()
            .unwrap_or(&self.spaces.workspace);
        if !scope.reaches(workspace) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!("workspace {workspace} is outside the workspaces you can use"),
            ));
        }
        Ok(Running {
            engine: self.engine.at(workspace),
            host: self.spaces.host(workspace),
            channel: self.spaces.channel.clone(),
        })
    }

    /// In every workspace, once memory answers: the open runs driven and
    /// the `event` triggers read. A workspace that cannot be read is said
    /// and does not hold the others.
    fn resume(self: &Arc<Self>) {
        let started = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match pcx_documents::every_workspace(&started.spaces.channel, &started.scope())
                    .await
                {
                    Ok(every) => {
                        for workspace in every {
                            match started.engine.at(&workspace).resume().await {
                                Ok(0) => {}
                                Ok(n) => {
                                    tracing::info!(%workspace, runs = n, "the open runs go on");
                                }
                                Err(e) => {
                                    tracing::warn!(%workspace, "the workflows of a workspace were not read: {e}");
                                }
                            }
                        }
                        return;
                    }
                    Err(e) => tracing::debug!("the workspaces cannot be read yet: {e}"),
                }
                tokio::select! {
                    () = started.stop.cancelled() => return,
                    () = tokio::time::sleep(AGAIN) => {}
                }
            }
        });
    }

    /// Follow the bus: each event told to the engine, then acknowledged.
    fn follow(self: &Arc<Self>) {
        let started = Arc::clone(self);
        let bus = self.spaces.host(&self.spaces.workspace);
        tokio::spawn(async move {
            loop {
                match bus.follow().await {
                    Ok(mut feed) => loop {
                        let next = tokio::select! {
                            () = started.stop.cancelled() => return,
                            next = feed.next() => next,
                        };
                        match next {
                            Some(Ok(Followed::Event(event))) => {
                                started.engine.heard(&event);
                                if let Err(e) = bus.ack(&event.name, event.position).await {
                                    tracing::warn!(event = %event.name, "an event was not acknowledged: {e}");
                                }
                            }
                            Some(Ok(Followed::FellBehind(name))) => {
                                tracing::warn!(event = %name, "events of this name were dropped before they were read");
                            }
                            Some(Err(e)) => {
                                tracing::warn!("the bus's feed ended: {e}");
                                break;
                            }
                            None => break,
                        }
                    },
                    Err(e) => tracing::debug!("the bus cannot be followed yet: {e}"),
                }
                tokio::select! {
                    () = started.stop.cancelled() => return,
                    () = tokio::time::sleep(AGAIN) => {}
                }
            }
        });
    }

    /// Stop driving the runs: they stay as their journals have them.
    fn stop(&self) {
        self.stop.cancel();
        self.engine.stop();
    }
}

/// The running module, shared by the module and its service; none until
/// it starts.
pub type Shared = Arc<Mutex<Option<Arc<Started>>>>;

/// The running module of `shared` in the workspace a call in `scope` is
/// asked from.
///
/// # Errors
/// `Unavailable` before it starts; [`Started::asked_from`]'s.
pub fn running(shared: &Shared, scope: &Scope) -> Result<Running, ModuleError> {
    let started = shared.lock().clone().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::Unavailable,
            "the workflow module has not started",
        )
    })?;
    started.asked_from(scope)
}

/// The `workflow` module.
pub struct WorkflowModule {
    manifest: Manifest,
    running: Shared,
}

impl WorkflowModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Workflows: graphs of steps over every module's tools, run when something happens, each step kept in a journal.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::WriteMemory.into(), Capability::Models.into()],
                config_type: WorkflowConfig::full_name(),
                document_kinds: specs::document_kinds(),
                uses: vec![EVERY_MODULE.to_string()],
                follows: vec![EVERY_MODULE.to_string()],
                emits: specs::emits(),
                services: vec![specs::SERVICE.to_string()],
                descriptors: post_cortex_proto::WORKFLOW_DESCRIPTOR_SET.to_vec(),
                rpcs: specs::rpcs(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for WorkflowModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for WorkflowModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: WorkflowConfig = ctx.config()?;
        if config.workspace_id.trim().is_empty() {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "the settings name the workspace",
            )
            .with_field("workspace_id"));
        }
        let spaces = Arc::new(HostSpaces {
            host: ctx.host,
            channel: ctx.channel,
            workspace: config.workspace_id,
            board: config.board,
        });
        let engine = Engine::of_spaces(spaces.clone(), &spaces.workspace)?;
        let started = Arc::new(Started {
            engine,
            spaces,
            stop: CancellationToken::new(),
        });
        started.resume();
        started.follow();
        if let Some(old) = self.running.lock().replace(started) {
            old.stop();
        }
        Ok(())
    }

    /// Its `pcx.workflow.v1.Workflows` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(
            WorkflowsServer::new(handlers),
        )))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("the workflow module serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(started) = self.running.lock().take() {
            started.stop();
        }
        Ok(())
    }
}
