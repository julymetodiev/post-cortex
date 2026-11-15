// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The core of a module in another language (docs/modules/dynamic.md, the
//! core): control is inverted. The core serves `pcx.module.v1` and the
//! module's own services on its runtime and never calls into the language;
//! the language asks for the next call ([`Handoff::next_call`]), runs its
//! handler and answers it ([`Handoff::complete`], [`Handoff::fail`]). Only
//! protobuf bytes and plain values cross.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pbjson_types::Any;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tonic::body::Body;
use tonic::codegen::http::Request;
use tonic::codegen::http::uri::PathAndQuery;

use crate::bus::{Bus, RemoteBus};
use crate::grpc::{GrpcChannel, Raw};
use crate::{
    Callback, Event, Manifest, ModuleError, ModuleErrorKind, Outcome, Process, Prompt,
    PromptContent, Resource, SOCKET_ENV, Scope, ServeError, Start,
};

mod declaration;

pub use declaration::{Declaration, Declared, Emitted, effect, failure, visibility};

/// Worker threads of the core's runtime.
const WORKERS: usize = 4;

/// A call handed to the language.
#[derive(Debug)]
pub struct Call {
    /// Its id, for the answer.
    pub id: u64,
    /// Its method's path.
    pub method: String,
    /// The request's bytes.
    pub body: Vec<u8>,
    /// Who and what it is for, its deadline and cancellation.
    pub scope: Scope,
}

/// Why the core failed.
#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    /// The host started the process without its socket.
    #[error("{SOCKET_ENV} is not set: a pcx module; the host starts it")]
    NoSocket,
    /// The runtime could not be made.
    #[error("the core's runtime: {0}")]
    Runtime(std::io::Error),
    /// The descriptors the host named could not be read.
    #[error("the module's descriptors: {0}")]
    Descriptors(std::io::Error),
    /// Serving failed.
    #[error(transparent)]
    Serve(#[from] ServeError),
    /// No call of this id waits for an answer (answered, or its caller
    /// gave up).
    #[error("no call {0} waits for an answer")]
    NoCall(u64),
}

/// A call waiting for its answer.
struct Waiting {
    answer: oneshot::Sender<Result<Vec<u8>, ModuleError>>,
    scope: Scope,
}

/// What the host gave at start.
struct Started {
    callback: Callback,
    config: Any,
}

struct Inner {
    manifest: Manifest,
    queue: mpsc::UnboundedSender<Call>,
    calls: tokio::sync::Mutex<mpsc::UnboundedReceiver<Call>>,
    waiting: Mutex<HashMap<u64, Waiting>>,
    next: AtomicU64,
    started: Mutex<Option<Arc<Started>>>,
    stopped: CancellationToken,
}

/// What names the file of a prepared module's descriptors, with their comments;
/// unset, or no such file: those the language declares.
pub const DESCRIPTORS_ENV: &str = "PCX_MODULE_DESCRIPTORS";

/// That file, in the directory of a prepared module's generated code.
pub const DESCRIPTORS_FILE: &str = "descriptors.binpb";

/// The core of one module process in another language.
#[derive(Clone)]
pub struct Handoff {
    inner: Arc<Inner>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl Handoff {
    /// The core of the module `declaration` declares, with its runtime.
    ///
    /// # Errors
    /// The runtime cannot be made, or the descriptors the host names do not read.
    pub fn new(mut declaration: Declaration) -> Result<Self, HandoffError> {
        if let Some(path) = std::env::var_os(DESCRIPTORS_ENV) {
            match std::fs::read(&path) {
                Ok(descriptors) => declaration.descriptors = descriptors,
                // A bundle prepared before the file was written, or without protos.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(HandoffError::Descriptors(e)),
            }
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKERS)
            .thread_name("pcx-module-core")
            .enable_all()
            .build()
            .map_err(HandoffError::Runtime)?;
        let (queue, calls) = mpsc::unbounded_channel();
        Ok(Self {
            inner: Arc::new(Inner {
                manifest: declaration.into(),
                queue,
                calls: tokio::sync::Mutex::new(calls),
                waiting: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
                started: Mutex::new(None),
                stopped: CancellationToken::new(),
            }),
            runtime: Arc::new(runtime),
        })
    }

    /// The runtime every future of the core runs on.
    #[must_use]
    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }

    /// Serve on the socket the host names in [`SOCKET_ENV`] until it stops
    /// the module; then [`Handoff::next_call`] answers none.
    ///
    /// # Errors
    /// No socket is named; serving failed.
    pub async fn serve(&self) -> Result<(), HandoffError> {
        let socket: PathBuf = std::env::var_os(SOCKET_ENV)
            .ok_or(HandoffError::NoSocket)?
            .into();
        let process = Arc::new(Handed(Arc::clone(&self.inner)));
        let served = crate::serve(process, &socket, tonic::service::Routes::builder()).await;
        self.inner.stopped.cancel();
        Ok(served?)
    }

    /// The next call, or none once the module stopped.
    pub async fn next_call(&self) -> Option<Call> {
        let mut calls = self.inner.calls.lock().await;
        tokio::select! {
            call = calls.recv() => call,
            () = self.inner.stopped.cancelled() => None,
        }
    }

    /// Answer call `id` with the response's bytes.
    ///
    /// # Errors
    /// [`HandoffError::NoCall`].
    pub fn complete(&self, id: u64, body: Vec<u8>) -> Result<(), HandoffError> {
        self.answer(id, Ok(body))
    }

    /// Fail call `id` with `error`.
    ///
    /// # Errors
    /// [`HandoffError::NoCall`].
    pub fn fail(&self, id: u64, error: ModuleError) -> Result<(), HandoffError> {
        self.answer(id, Err(error))
    }

    fn answer(&self, id: u64, answer: Result<Vec<u8>, ModuleError>) -> Result<(), HandoffError> {
        let waiting = self
            .inner
            .waiting
            .lock()
            .remove(&id)
            .ok_or(HandoffError::NoCall(id))?;
        // Its caller gave up (its deadline) while the handler ran.
        waiting
            .answer
            .send(answer)
            .map_err(|_| HandoffError::NoCall(id))
    }

    /// Whether call `id` still waits for its answer: false once it is
    /// answered or its caller gave up (its deadline passed), which a
    /// handler reads as its cancellation.
    #[must_use]
    pub fn waits(&self, id: u64) -> bool {
        self.inner.waiting.lock().contains_key(&id)
    }

    /// The scope of call `id`, while it waits for its answer.
    fn scope(&self, id: u64) -> Result<Scope, ModuleError> {
        self.inner
            .waiting
            .lock()
            .get(&id)
            .map(|w| w.scope.clone())
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!("no call {id} waits for an answer"),
                )
            })
    }

    fn started(&self) -> Result<Arc<Started>, ModuleError> {
        self.inner.started.lock().clone().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "the host has not started the module",
            )
        })
    }

    /// Call `path` of a module this one uses with the request's bytes, in
    /// call `id`'s scope; the answer's bytes.
    ///
    /// # Errors
    /// `FailedPrecondition` outside a waiting call or before the start;
    /// `InvalidArgument` for a path that is not one; the call's failure.
    pub async fn host_call(
        &self,
        id: u64,
        path: &str,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, ModuleError> {
        let scope = self.scope(id)?;
        let channel = self.started()?.callback.grpc();
        let path = PathAndQuery::try_from(path).map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("{path} is not a method's path: {e}"),
            )
        })?;
        let mut request = tonic::Request::new(body);
        scope.to_metadata(request.metadata_mut())?;
        request.set_timeout(scope.remaining());
        crate::grpc::unary(channel, path, request).await
    }

    /// Emit the event `name` the module declares, its payload a `message`
    /// (its full name) of `body`, in call `id`'s scope; the event's id.
    ///
    /// # Errors
    /// As [`Bus::publish`]; `FailedPrecondition` outside a waiting call,
    /// before the start, or when the host gave no callback socket.
    pub async fn publish(
        &self,
        id: u64,
        name: &str,
        message: &str,
        body: Vec<u8>,
    ) -> Result<String, ModuleError> {
        let scope = self.scope(id)?;
        let bus = RemoteBus::new(self.started()?.callback.channel()?);
        let payload = Any {
            type_url: format!("type.googleapis.com/{message}"),
            value: body.into(),
        };
        bus.publish(&scope, name, payload).await
    }

    /// The configuration the host started the module with: its message
    /// type and bytes (empty: it takes none).
    ///
    /// # Errors
    /// `FailedPrecondition` before the start.
    pub fn config(&self) -> Result<(String, Vec<u8>), ModuleError> {
        let config = &self.started()?.config;
        if config.type_url.is_empty() {
            return Ok((String::new(), config.value.to_vec()));
        }
        let message = crate::bus::payload_type(config).ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!("the host gave a configuration of type {}", config.type_url),
            )
        })?;
        Ok((message.to_string(), config.value.to_vec()))
    }
}

/// The module the host sees: each call of its services is handed over.
struct Handed(Arc<Inner>);

#[async_trait]
impl Process for Handed {
    fn manifest(&self) -> &Manifest {
        &self.0.manifest
    }

    async fn start(&self, start: Start) -> Result<(), ModuleError> {
        *self.0.started.lock() = Some(Arc::new(Started {
            callback: Callback::new(&start.host_socket),
            config: start.config,
        }));
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("{} serves no resource {uri}", self.0.manifest.id),
        ))
    }

    async fn prompts(&self, _: &Scope) -> Result<Vec<Prompt>, ModuleError> {
        Ok(Vec::new())
    }

    async fn prompt(
        &self,
        _: &Scope,
        name: &str,
        _: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("{} offers no prompt {name}", self.0.manifest.id),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        self.0.stopped.cancel();
        Ok(())
    }

    fn services(&self) -> Option<GrpcChannel> {
        let inner = Arc::clone(&self.0);
        Some(crate::grpc::channel(tower::service_fn(
            move |request: Request<Body>| {
                let inner = Arc::clone(&inner);
                async move {
                    let method = request.uri().path().to_string();
                    let mut server = tonic::server::Grpc::new(Raw);
                    Ok::<_, std::convert::Infallible>(
                        server.unary(Unary { inner, method }, request).await,
                    )
                }
            },
        )))
    }
}

/// One call of the module's services, handed over and awaited.
struct Unary {
    inner: Arc<Inner>,
    method: String,
}

impl tonic::server::UnaryService<Vec<u8>> for Unary {
    type Response = Vec<u8>;
    type Future =
        Pin<Box<dyn Future<Output = Result<tonic::Response<Vec<u8>>, Status>> + Send + 'static>>;

    fn call(&mut self, request: tonic::Request<Vec<u8>>) -> Self::Future {
        let inner = Arc::clone(&self.inner);
        let method = std::mem::take(&mut self.method);
        Box::pin(async move {
            let scope = Scope::of(&request)?;
            let remaining = scope.remaining();
            let id = inner.next.fetch_add(1, Ordering::Relaxed);
            let (answer, answered) = oneshot::channel();
            inner.waiting.lock().insert(
                id,
                Waiting {
                    answer,
                    scope: scope.clone(),
                },
            );
            let call = Call {
                id,
                method,
                body: request.into_inner(),
                scope: scope.clone(),
            };
            if inner.queue.send(call).is_err() {
                inner.waiting.lock().remove(&id);
                return Err(Status::unavailable("the module stopped"));
            }
            let answer =
                tokio::time::timeout(remaining.max(Duration::from_millis(1)), answered).await;
            // Done or not, the call no longer waits; the handler's late
            // answer finds nothing.
            let answer = match answer {
                Ok(Ok(answer)) => answer.map_err(Status::from),
                Ok(Err(_)) => Err(Status::internal("the handler dropped the call")),
                Err(_) => {
                    inner.waiting.lock().remove(&id);
                    scope.cancel.cancel();
                    Err(Status::deadline_exceeded(
                        "the handler did not answer in time",
                    ))
                }
            };
            answer.map(tonic::Response::new)
        })
    }
}
