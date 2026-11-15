// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The out-of-process runner (ARCHITECTURE section 7): a module in its own
//! process, any language, serving `pcx.module.v1` over gRPC on a Unix
//! socket the host names in `PCX_MODULE_SOCKET`. The host launches it,
//! reads its manifest, starts it with its configuration, calls its tools,
//! sends its events on one stream, describes it without starting it (a
//! catalog reads its manifest), restarts it with a backoff when it exits
//! on its own (T4), and stops it. Its executable is pinned by SHA-256 like
//! a hook script's. A process of a slot serves its port on its socket: a
//! model provider its models, a context engine its engine, an agent its
//! turns, which the host reaches as an in-process module's.

mod callback;
mod direct;
mod hooks_service;
mod launcher;
mod link;
mod port;
mod sandbox_service;
mod supervise;

use std::collections::HashMap;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::agent::Turns;
use pcx_module_api::context::ContextEngine;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::model::ModelProvider;
use pcx_module_api::{
    Event, HostContext, LogLine, LogRing, Manifest, Module, ModuleError, ModuleErrorKind, Outcome,
    Prompt, PromptContent, Resource, Scope,
};
use post_cortex_proto::module as wire;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::grant::{self, Grant, RootGrant};
use crate::process::callback::Callback;
use crate::process::link::{Link, Waiting};
use crate::process::port::{Port, port};
use crate::process::supervise::{Supervised, exit, launch, remove_sockets, supervise, with_output};
use crate::{HostError, pin, registry};

pub use direct::Direct;
pub use launcher::{Ended, Launch, ModuleLauncher, ModuleProcess, output_lines};

/// Launches by this process so far, which name their sockets apart.
static LAUNCHES: AtomicU64 = AtomicU64::new(0);

/// An out-of-process module as the configuration declares it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSpec {
    /// The module id; its manifest must name the same.
    pub id: String,
    /// The executable.
    pub command: PathBuf,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// The approved SHA-256 of the executable's file, in hex.
    pub sha256: String,
    /// The module's configuration (a table), given to it at start.
    #[serde(default)]
    pub config: Option<Value>,
    /// The capabilities granted to it; one its manifest asks for that is
    /// not granted refuses it.
    #[serde(default)]
    pub grants: Vec<Grant>,
    /// The modules whose tools it may call; one its manifest uses that is
    /// not named refuses it.
    #[serde(default)]
    pub uses: Vec<String>,
    /// The directories it may use (with the `filesystem` grant): the roots
    /// of its sandbox.
    #[serde(default)]
    pub filesystem: Vec<RootGrant>,
}

/// A module running in its own process.
pub struct ProcessModule {
    spec: ProcessSpec,
    /// This launch's directory, its sockets' alone (the only part of the
    /// host a sandboxed module shares).
    dir: PathBuf,
    socket: PathBuf,
    /// Where the host serves the module's callbacks once it starts.
    host_socket: PathBuf,
    callback: Mutex<Option<Callback>>,
    manifest: Manifest,
    link: Arc<Link>,
    /// The process until the host starts the module; then its supervisor
    /// has it.
    child: Mutex<Option<Box<dyn ModuleProcess>>>,
    launcher: Arc<dyn ModuleLauncher>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    stopping: CancellationToken,
    seq: AtomicU64,
    /// The port of the slot it fills, on its socket.
    port: Port,
    /// Its gRPC services on its socket, when its manifest names any.
    services: Option<GrpcChannel>,
    /// What its process wrote and the host saw of the process.
    log: Arc<LogRing>,
}

impl ProcessModule {
    /// Launch `spec`'s executable directly (a first-party module) serving
    /// on a socket under `socket_dir` and read its manifest.
    ///
    /// # Errors
    /// As [`ProcessModule::launch_by`].
    pub async fn launch(spec: &ProcessSpec, socket_dir: &Path) -> Result<Self, HostError> {
        Self::launch_by(spec, socket_dir, Arc::new(Direct)).await
    }

    /// Launch `spec`'s executable through `launcher`, serving on a socket
    /// in a directory of its own under `socket_dir`, and read its manifest.
    ///
    /// # Errors
    /// The executable is not the approved one, does not start, does not
    /// serve its socket in time, names another id, or asks for a capability
    /// or uses a module not granted.
    pub async fn launch_by(
        spec: &ProcessSpec,
        socket_dir: &Path,
        launcher: Arc<dyn ModuleLauncher>,
    ) -> Result<Self, HostError> {
        if launcher.pins() {
            pin::at_load(&spec.id, &spec.command, &spec.sha256)?;
        }
        // Unique per launch: one process may run the same module twice (a
        // host per tenant, or a reload starting a new configuration before
        // the old one stops).
        let launch_no = LAUNCHES.fetch_add(1, Ordering::Relaxed);
        let dir = socket_dir.join(format!(
            "pcx-{}-{launch_no}-{}",
            std::process::id(),
            spec.id
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| HostError::Launch {
                module: spec.id.clone(),
                reason: format!("make {}: {e}", dir.display()),
            })?;
        let socket = dir.join("m.sock");
        let log = Arc::new(LogRing::default());
        let launched = match launch(spec, &socket, &log, launcher.as_ref()).await {
            Ok(launched) => launched,
            Err(reason) => {
                remove_sockets(&dir, &spec.id);
                return Err(HostError::Launch {
                    module: spec.id.clone(),
                    reason: with_output(reason, &log).await,
                });
            }
        };
        let channel = launched.channel.clone();
        // Built first, so a refused one takes its process and socket along.
        let mut module = Self {
            spec: spec.clone(),
            host_socket: dir.join("h.sock"),
            callback: Mutex::new(None),
            dir,
            socket,
            manifest: launched.manifest,
            link: Arc::new(Link::new(launched.client)),
            child: Mutex::new(Some(launched.child)),
            launcher,
            supervisor: Mutex::new(None),
            stopping: CancellationToken::new(),
            seq: AtomicU64::new(0),
            port: Port::None,
            services: None,
            log,
        };
        if module.manifest.id != spec.id {
            return Err(HostError::InvalidManifest {
                module: spec.id.clone(),
                reason: format!("the process names itself {}", module.manifest.id),
            });
        }
        if let Some(capability) = grant::missing(&module.manifest, &spec.grants) {
            return Err(HostError::NotGranted {
                module: spec.id.clone(),
                capability,
            });
        }
        if let Some(used) = module
            .manifest
            .uses
            .iter()
            .find(|used| !spec.uses.contains(used))
        {
            return Err(HostError::UseNotGranted {
                module: spec.id.clone(),
                uses: used.clone(),
            });
        }
        // The channel reaches a restarted process too: it binds the same
        // socket.
        module.services =
            (!module.manifest.services.is_empty()).then(|| grpc::channel(channel.clone()));
        module.port =
            port(&module.manifest, channel)
                .await
                .map_err(|reason| HostError::Launch {
                    module: spec.id.clone(),
                    reason,
                })?;
        Ok(module)
    }
}

impl ProcessModule {
    /// The manifest of `spec`'s module, read without starting it: launch
    /// the process, read and check its manifest (its id, the grants it
    /// needs, its configuration schema), stop it.
    ///
    /// # Errors
    /// As [`ProcessModule::launch`], or the host refuses the manifest.
    pub async fn describe(spec: &ProcessSpec, socket_dir: &Path) -> Result<Manifest, HostError> {
        Self::describe_by(spec, socket_dir, Arc::new(Direct))
            .await
            .map(|(manifest, _)| manifest)
    }

    /// [`ProcessModule::describe`], launched by `launcher`; with what its
    /// process wrote.
    ///
    /// # Errors
    /// As [`ProcessModule::describe`].
    pub async fn describe_by(
        spec: &ProcessSpec,
        socket_dir: &Path,
        launcher: Arc<dyn ModuleLauncher>,
    ) -> Result<(Manifest, Vec<LogLine>), HostError> {
        let module = Self::launch_by(spec, socket_dir, launcher).await?;
        let checked = registry::check_manifest(&pcx_module_api::API_VERSION, &module.manifest);
        let output = module.log.lines();
        if let Err(e) = module.stop().await {
            tracing::warn!(module = spec.id, error = %e, "described module did not stop cleanly");
        }
        checked?;
        Ok((module.manifest.clone(), output))
    }
}

impl Drop for ProcessModule {
    /// A module dropped without a stop takes its process and its socket
    /// along (a reload that refused it before its start).
    fn drop(&mut self) {
        if let Some(supervisor) = self.supervisor.lock().take() {
            supervisor.abort();
        }
        if let Some(callback) = self.callback.lock().take() {
            callback.abort();
        }
        remove_sockets(&self.dir, &self.spec.id);
    }
}

/// A request that carries the scope's deadline.
fn request<T>(message: T, scope: &Scope) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.set_timeout(scope.remaining());
    request
}

fn unavailable(message: &str) -> ModuleError {
    ModuleError::new(ModuleErrorKind::Unavailable, message)
}

#[async_trait]
impl Module for ProcessModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn model_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        match &self.port {
            Port::Models(models) => Some(Arc::clone(models) as Arc<dyn ModelProvider>),
            _ => None,
        }
    }

    fn context_engine(&self) -> Option<Arc<dyn ContextEngine>> {
        match &self.port {
            Port::Context(engine) => Some(Arc::clone(engine)),
            _ => None,
        }
    }

    fn turns(&self) -> Option<Arc<dyn Turns>> {
        match &self.port {
            Port::Turns(turns) => Some(Arc::clone(turns)),
            _ => None,
        }
    }

    /// Its gRPC services (W23), on its socket: the host forwards their
    /// calls to the process.
    fn services(&self) -> Option<GrpcChannel> {
        self.services.clone()
    }

    fn output(&self) -> Vec<LogLine> {
        self.log.lines()
    }

    /// Start the process, then keep it running.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let events = !self.manifest.events.is_empty();
        let callback = Callback::serve(
            Arc::clone(&ctx.host),
            ctx.channel.clone(),
            &self.host_socket,
            &self.spec.id,
        )
        .map_err(|reason| unavailable(&reason))?;
        *self.callback.lock() = Some(callback);
        self.link
            .start(
                self.link.client(),
                ctx,
                events,
                &self.spec.id,
                &self.host_socket,
            )
            .await?;
        // A provider has its settings once started: its models now (its
        // table's), then its API's listing once it is in,
        // which a start does not wait on.
        if let Port::Models(models) = &self.port {
            models.refresh(false).await?;
            let (models, module) = (Arc::clone(models), self.spec.id.clone());
            tokio::spawn(async move {
                if let Err(error) = models.refresh(true).await {
                    tracing::warn!(module, error = %error, "its models were not listed");
                }
            });
        }
        let child = self
            .child
            .lock()
            .take()
            .ok_or_else(|| unavailable("the module was started before"))?;
        let supervised = Supervised {
            spec: self.spec.clone(),
            socket: self.socket.clone(),
            host_socket: self.host_socket.clone(),
            link: Arc::clone(&self.link),
            events,
            stopping: self.stopping.clone(),
            log: Arc::clone(&self.log),
            launcher: Arc::clone(&self.launcher),
        };
        *self.supervisor.lock() = Some(tokio::spawn(supervise(child, supervised)));
        Ok(())
    }

    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        let sender = self
            .link
            .events()
            .ok_or_else(|| unavailable("the module's event stream is not open"))?;
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        let pending = self.link.pending();
        pending.lock().insert(seq, answer);
        let _waiting = Waiting { pending, seq };
        let envelope = wire::EventEnvelope {
            seq,
            scope: Some(scope.to_wire()),
            event: Some(event.clone()),
        };
        sender
            .send(envelope)
            .await
            .map_err(|_| unavailable("the module's event stream ended"))?;
        answered
            .await
            .map_err(|_| unavailable("the module's event stream ended"))?
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        let request = wire::GetResourceRequest {
            uri: uri.to_string(),
        };
        let response = self
            .link
            .client()
            .get_resource(request)
            .await
            .map_err(ModuleError::from)?
            .into_inner();
        match response.result {
            Some(wire::get_resource_response::Result::Resource(resource)) => Ok(resource),
            Some(wire::get_resource_response::Result::Error(error)) => Err(error.into()),
            None => Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "GetResource answered without a resource or an error",
            )),
        }
    }

    async fn prompts(&self, scope: &Scope) -> Result<Vec<Prompt>, ModuleError> {
        let listing = wire::ListPromptsRequest {
            scope: Some(scope.to_wire()),
        };
        let response = self
            .link
            .client()
            .list_prompts(request(listing, scope))
            .await
            .map_err(ModuleError::from)?
            .into_inner();
        match response.result {
            Some(wire::list_prompts_response::Result::Prompts(list)) => Ok(list.prompts),
            Some(wire::list_prompts_response::Result::Error(error)) => Err(error.into()),
            None => Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "ListPrompts answered without prompts or an error",
            )),
        }
    }

    async fn prompt(
        &self,
        scope: &Scope,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        let asked = wire::GetPromptRequest {
            scope: Some(scope.to_wire()),
            name: name.to_string(),
            arguments: arguments.clone(),
        };
        let response = self
            .link
            .client()
            .get_prompt(request(asked, scope))
            .await
            .map_err(ModuleError::from)?
            .into_inner();
        match response.result {
            Some(wire::get_prompt_response::Result::Prompt(prompt)) => Ok(prompt),
            Some(wire::get_prompt_response::Result::Error(error)) => Err(error.into()),
            None => Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "GetPrompt answered without a prompt or an error",
            )),
        }
    }

    /// Stop the module and wait for its process to exit (killed past the
    /// exit deadline); it is not restarted. A module never started is
    /// stopped the same way.
    async fn stop(&self) -> Result<(), ModuleError> {
        // First, so the process's own exit is not taken for a crash.
        self.stopping.cancel();
        self.link.close_events();
        let stopped = self.link.client().stop(wire::StopRequest {}).await;
        let supervisor = self.supervisor.lock().take();
        if let Some(supervisor) = supervisor
            && let Err(e) = supervisor.await
        {
            tracing::warn!(module = self.spec.id, error = %e, "module supervisor");
        }
        let unstarted = self.child.lock().take();
        if let Some(child) = unstarted {
            exit(child, &self.spec.id).await;
        }
        let callback = self.callback.lock().take();
        if let Some(callback) = callback {
            callback.stop(&self.spec.id).await;
        }
        remove_sockets(&self.dir, &self.spec.id);
        match stopped.map_err(ModuleError::from)?.into_inner().error {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }
}
