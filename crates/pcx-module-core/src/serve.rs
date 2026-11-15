// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The server of a module process: [`serve`] runs a [`Process`] as the
//! `pcx.module.v1.Module` gRPC service on the Unix socket the host names
//! in `PCX_MODULE_SOCKET`, until the host stops it, with its own services
//! and those of the slot it fills (given as routes) on the same socket.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pbjson_types::Any;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::module_server::{Module as ModuleService, ModuleServer};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tokio_util::sync::CancellationToken;
use tonic::service::RoutesBuilder;
use tonic::{Request, Response, Status, Streaming};

use crate::grpc::GrpcChannel;
use crate::{
    ApiVersion, Event, Manifest, ModuleError, ModuleErrorKind, Outcome, Prompt, PromptContent,
    Resource, Scope,
};

/// The environment variable that names the socket a module serves on.
pub const SOCKET_ENV: &str = "PCX_MODULE_SOCKET";

/// Longest a call over `pcx.module.v1` may run when its request sets no
/// deadline (a module serving the host, the host serving a module's
/// callbacks).
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);

/// Events a module works on at once before the stream waits.
const EVENT_QUEUE: usize = 64;

/// Why a module could not be served.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The socket could not be bound.
    #[error("cannot bind {path}: {source}")]
    Bind {
        /// The socket.
        path: String,
        /// Why.
        source: std::io::Error,
    },
    /// The bound socket could not be given its rights.
    #[error("cannot give {path} its rights: {source}")]
    Rights {
        /// The socket.
        path: String,
        /// Why.
        source: std::io::Error,
    },
    /// The server failed.
    #[error("module server: {0}")]
    Server(#[from] tonic::transport::Error),
}

/// What the host gives a module process when it starts it.
#[derive(Debug, Clone)]
pub struct Start {
    /// The module API version the host speaks.
    pub host_api: ApiVersion,
    /// The module's configuration, a message of its `config_type` (empty:
    /// it takes none), which the host has checked.
    pub config: Any,
    /// The callback socket (empty: none), for [`crate::Callback`].
    pub host_socket: String,
}

/// A module as its process serves it (`pcx.module.v1.Module`).
#[async_trait]
pub trait Process: Send + Sync {
    /// What the module is; read before [`Process::start`].
    fn manifest(&self) -> &Manifest;

    /// Start the module; nothing else is called until it returns.
    ///
    /// # Errors
    /// The module cannot run (its configuration or a backend).
    async fn start(&self, start: Start) -> Result<(), ModuleError>;

    /// Answer an event the module subscribed to.
    ///
    /// # Errors
    /// See [`ModuleError`]; the host applies the subscription's policy.
    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError>;

    /// The HTML resource at `uri`, one of its views' `ui://` URIs.
    ///
    /// # Errors
    /// `NotFound` for a URI it does not serve.
    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError>;

    /// The prompts it offers now, in its order.
    ///
    /// # Errors
    /// See [`ModuleError`].
    async fn prompts(&self, scope: &Scope) -> Result<Vec<Prompt>, ModuleError>;

    /// Its prompt `name`, filled with `arguments`.
    ///
    /// # Errors
    /// `NotFound` for a name it does not offer; `InvalidArgument` for
    /// arguments the prompt does not take.
    async fn prompt(
        &self,
        scope: &Scope,
        name: &str,
        arguments: &std::collections::HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError>;

    /// Stop the module; it may finish what is in flight.
    ///
    /// # Errors
    /// It did not stop cleanly.
    async fn stop(&self) -> Result<(), ModuleError>;

    /// Its gRPC services (`Manifest.services`, W23) as one channel: every
    /// call the other services of its socket do not take; none for a
    /// module without.
    fn services(&self) -> Option<GrpcChannel>;
}

/// A module process's logs (`RUST_LOG`, else `info`) on stderr, which its
/// host keeps; `name` is the executable's, for the message when they
/// cannot be set up.
pub fn logs(name: &str) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info"));
    match filter {
        Ok(filter) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .init(),
        Err(e) => eprintln!("{name}: no logs: {e}"),
    }
}

/// Serve `process` on the Unix socket `socket` until the host calls Stop,
/// with `slots` (the services of the slot it fills; empty: none) beside
/// `pcx.module.v1.Module`, and its own services taking every other call.
///
/// # Errors
/// The socket cannot be bound, or the server fails.
pub async fn serve(
    process: Arc<dyn Process>,
    socket: &Path,
    mut slots: RoutesBuilder,
) -> Result<(), ServeError> {
    let listener = UnixListener::bind(socket).map_err(|source| ServeError::Bind {
        path: socket.display().to_string(),
        source,
    })?;
    // Explicit rights, no umask: its user's and its directory's group's,
    // which on a cloud is its host's (docs/modules/dynamic.md, the host's
    // sockets).
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660)).map_err(|source| {
        ServeError::Rights {
            path: socket.display().to_string(),
            source,
        }
    })?;
    let stopped = CancellationToken::new();
    tokio::spawn(follow_host(
        std::os::unix::process::parent_id(),
        stopped.clone(),
    ));
    let services = process.services();
    slots.add_service(ModuleServer::new(Served {
        process,
        stopped: stopped.clone(),
    }));
    let routes = match services {
        Some(services) => crate::grpc::with_fallback(slots.routes(), services),
        None => slots.routes(),
    };
    tonic::transport::Server::builder()
        .add_routes(routes)
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), stopped.cancelled())
        .await?;
    Ok(())
}

/// How often a module process looks whether its host is still there.
const HOST_CHECK: std::time::Duration = std::time::Duration::from_secs(1);

/// A module process is its host's: it stops when the host calls Stop, or
/// when the host is gone (its parent is no longer `host`). A stop signal sent
/// to every process of a jail or a process group (SIGTERM, SIGINT, SIGHUP:
/// a Nomad stop, a terminal) leaves the stopping to the host, which stops
/// its modules in order.
async fn follow_host(host: u32, stopped: CancellationToken) {
    use tokio::signal::unix::{SignalKind, signal};
    let signals = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    );
    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = signals else {
        tracing::warn!("a stop signal cannot be left to the host: it stops this module");
        return;
    };
    let mut check = tokio::time::interval(HOST_CHECK);
    loop {
        tokio::select! {
            () = stopped.cancelled() => return,
            _ = term.recv() => tracing::debug!("SIGTERM: the host stops this module"),
            _ = int.recv() => tracing::debug!("SIGINT: the host stops this module"),
            _ = hup.recv() => tracing::debug!("SIGHUP: the host stops this module"),
            _ = check.tick() => {
                if std::os::unix::process::parent_id() != host {
                    tracing::info!("the host is gone: this module stops");
                    stopped.cancel();
                    return;
                }
            }
        }
    }
}

/// A process behind the generated service.
struct Served {
    process: Arc<dyn Process>,
    /// Cancelled once Stop has answered: the server shuts down.
    stopped: CancellationToken,
}

/// The scope of a request, its deadline counted from now.
fn scope(scope: Option<wire::Scope>) -> Result<Scope, Status> {
    let scope = scope.ok_or_else(|| Status::invalid_argument("the request has no scope"))?;
    // A dropped request cancels its handler; the token is the module's view.
    Ok(Scope::from_wire(
        scope,
        CancellationToken::new(),
        DEFAULT_DEADLINE,
    ))
}

#[tonic::async_trait]
impl ModuleService for Served {
    async fn get_manifest(
        &self,
        _request: Request<wire::GetManifestRequest>,
    ) -> Result<Response<wire::Manifest>, Status> {
        Ok(Response::new(self.process.manifest().clone()))
    }

    async fn start(
        &self,
        request: Request<wire::StartRequest>,
    ) -> Result<Response<wire::StartResponse>, Status> {
        let request = request.into_inner();
        let host_api = request
            .host_api
            .ok_or_else(|| Status::invalid_argument("Start names no host API version"))?;
        let start = Start {
            host_api,
            // Unset: the module takes no configuration.
            config: request.config.unwrap_or_default(),
            host_socket: request.host_socket,
        };
        let error = self
            .process
            .start(start)
            .await
            .err()
            .map(wire::ModuleError::from);
        Ok(Response::new(wire::StartResponse { error }))
    }

    type EventsStream = ReceiverStream<Result<wire::OutcomeEnvelope, Status>>;

    /// Each event is answered as it is done, matched to it by `seq`.
    async fn events(
        &self,
        request: Request<Streaming<wire::EventEnvelope>>,
    ) -> Result<Response<Self::EventsStream>, Status> {
        let mut events = request.into_inner();
        let (answers, stream) = mpsc::channel(EVENT_QUEUE);
        let process = Arc::clone(&self.process);
        tokio::spawn(async move {
            while let Ok(Some(envelope)) = events.message().await {
                let process = Arc::clone(&process);
                let answers = answers.clone();
                tokio::spawn(async move {
                    let result = answer(process.as_ref(), envelope.scope, envelope.event).await;
                    let outcome = wire::OutcomeEnvelope {
                        seq: envelope.seq,
                        result: Some(match result {
                            Ok(outcome) => wire::outcome_envelope::Result::Outcome(outcome),
                            Err(error) => wire::outcome_envelope::Result::Error(error.into()),
                        }),
                    };
                    // The host closed the stream: nobody waits for it.
                    if answers.send(Ok(outcome)).await.is_err() {
                        tracing::debug!(
                            seq = envelope.seq,
                            "event answered after the stream closed"
                        );
                    }
                });
            }
        });
        Ok(Response::new(ReceiverStream::new(stream)))
    }

    async fn get_resource(
        &self,
        request: Request<wire::GetResourceRequest>,
    ) -> Result<Response<wire::GetResourceResponse>, Status> {
        let uri = request.into_inner().uri;
        let result = match self.process.resource(&uri).await {
            Ok(resource) => wire::get_resource_response::Result::Resource(resource),
            Err(error) => wire::get_resource_response::Result::Error(error.into()),
        };
        Ok(Response::new(wire::GetResourceResponse {
            result: Some(result),
        }))
    }

    async fn list_prompts(
        &self,
        request: Request<wire::ListPromptsRequest>,
    ) -> Result<Response<wire::ListPromptsResponse>, Status> {
        let scope = scope(request.into_inner().scope)?;
        let result = match self.process.prompts(&scope).await {
            Ok(prompts) => {
                wire::list_prompts_response::Result::Prompts(wire::PromptList { prompts })
            }
            Err(error) => wire::list_prompts_response::Result::Error(error.into()),
        };
        Ok(Response::new(wire::ListPromptsResponse {
            result: Some(result),
        }))
    }

    async fn get_prompt(
        &self,
        request: Request<wire::GetPromptRequest>,
    ) -> Result<Response<wire::GetPromptResponse>, Status> {
        let request = request.into_inner();
        let scope = scope(request.scope)?;
        let result = match self
            .process
            .prompt(&scope, &request.name, &request.arguments)
            .await
        {
            Ok(prompt) => wire::get_prompt_response::Result::Prompt(prompt),
            Err(error) => wire::get_prompt_response::Result::Error(error.into()),
        };
        Ok(Response::new(wire::GetPromptResponse {
            result: Some(result),
        }))
    }

    async fn stop(
        &self,
        _request: Request<wire::StopRequest>,
    ) -> Result<Response<wire::StopResponse>, Status> {
        let error = self.process.stop().await.err().map(wire::ModuleError::from);
        self.stopped.cancel();
        Ok(Response::new(wire::StopResponse { error }))
    }
}

/// The process's outcome of one event envelope.
async fn answer(
    process: &dyn Process,
    scope: Option<wire::Scope>,
    event: Option<wire::Event>,
) -> Result<wire::Outcome, ModuleError> {
    let invalid = |what: &str| {
        ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("the event envelope has no {what}"),
        )
    };
    let scope = scope.ok_or_else(|| invalid("scope"))?;
    let event = event.ok_or_else(|| invalid("event"))?;
    let scope = Scope::from_wire(scope, CancellationToken::new(), DEFAULT_DEADLINE);
    process.on_event(&scope, &event).await
}
