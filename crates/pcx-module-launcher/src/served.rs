// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The launcher of the pot `pcx-modules` (`pcx module-launcher`,
//! `pcx.module.v1.Launcher`, docs/modules/dynamic.md): it starts what a host
//! asks, only one of its runtimes (a dynamic module) or of its programs (a
//! module of the cloud's catalog, its SHA-256 checked at each launch),
//! reading only under its stores, serving a socket only under the shared
//! directory, without other processes, and without network but for a
//! program its configuration grants it. A process lives as long as its host
//! reads its events.

use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use pcx_host::{Launch, ModuleLauncher, ModuleProcess};
use pcx_sandbox::Launcher;
use post_cortex_proto::module::launch_event::Event;
use post_cortex_proto::module::launcher_server::{self, LauncherServer};
use post_cortex_proto::module::{
    LaunchEnded, LaunchEvent, LaunchRequest, LaunchStarted, PrepareRequest, PrepareResponse,
};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tonic::{Request, Response, Status};

use crate::confined::{Confined, Confinement, share};
use crate::prepare::{PrepareError, Preparer};
use crate::watch::Watcher;

/// Events of a process buffered before it waits for its host.
const EVENTS: usize = 256;

/// How long a stopping launcher waits for its modules: within a job's kill
/// timeout.
const MODULES_STOP: std::time::Duration = std::time::Duration::from_secs(20);

/// The largest message: a bundle of 16 MiB and its envelope.
pub const MESSAGE_BYTES: usize = 17 * 1024 * 1024;

/// What a launcher starts and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedConfig {
    /// The runtimes it starts a dynamic module with.
    pub runtimes: Vec<PathBuf>,
    /// The programs of the cloud's catalog it starts without the network.
    pub programs: Vec<PathBuf>,
    /// Those it starts with the network (docs/host.md, network).
    pub network_programs: Vec<PathBuf>,
    /// What every process reads: the runtimes and the bindings.
    pub always_read: Vec<PathBuf>,
    /// Under what a process may read besides.
    pub stores: Vec<PathBuf>,
    /// The directory it shares with the host, of the launches' sockets.
    pub sockets: PathBuf,
    /// Where the modules' scratch directories are.
    pub scratch: PathBuf,
    /// The group of the hosts it serves when they do not run as root (the
    /// cloud's user's): its socket, `sockets` and each launch's directory
    /// are theirs too; none: its socket is its user's alone.
    pub host_group: Option<u32>,
}

/// The launcher.
pub struct Served {
    /// Cancelled when the launcher stops: its modules stop first.
    stopping: CancellationToken,
    /// The modules it follows.
    followed: TaskTracker,
    config: ServedConfig,
    preparer: Preparer,
    launcher: Launcher,
    watcher: Arc<Watcher>,
}

impl Served {
    /// A launcher of `config`, preparing bundles by `preparer` and confining
    /// by `launcher`. Call within a runtime.
    #[must_use]
    pub fn new(config: ServedConfig, preparer: Preparer, launcher: Launcher) -> Self {
        Self {
            stopping: CancellationToken::new(),
            followed: TaskTracker::new(),
            config,
            preparer,
            launcher,
            watcher: Watcher::start(),
        }
    }

    /// `request` as this launcher starts it.
    fn confinement(&self, request: &LaunchRequest) -> Result<(Launch, Confinement), Status> {
        let c = &self.config;
        name(&request.module, "module")?;
        name(&request.owner, "owner")?;
        let program = PathBuf::from(&request.program);
        let socket = PathBuf::from(&request.socket);
        let launch_dir = socket.parent().filter(|dir| clean(dir));
        if launch_dir.and_then(Path::parent) != Some(c.sockets.as_path()) {
            return Err(refused(format!(
                "{} is not a launch's socket under {}",
                request.socket,
                c.sockets.display()
            )));
        }
        if request.memory_mb == 0 || request.open_files == 0 {
            return Err(refused(format!(
                "{}: a module has limits on memory and open files",
                request.module
            )));
        }
        let scratch = c.scratch.join(&request.owner).join(&request.module);
        let program_network = if c.programs.contains(&program) {
            Some(false)
        } else if c.network_programs.contains(&program) {
            Some(true)
        } else {
            None
        };
        let (read, cwd, network) = if c.runtimes.contains(&program) {
            let (read, cwd) = self.bundle(request)?;
            (read, cwd, false)
        } else if let Some(network) = program_network {
            if !request.read.is_empty() || !request.cwd.is_empty() {
                return Err(refused(format!(
                    "{}: a program reads only the system's files",
                    request.program
                )));
            }
            pcx_host::at_load(&request.module, &program, &request.sha256)
                .map_err(|e| refused(e.to_string()))?;
            (Vec::new(), scratch.clone(), network)
        } else {
            return Err(refused(format!(
                "{} is neither a runtime nor a program of this launcher",
                request.program
            )));
        };
        let launch = Launch {
            id: request.module.clone(),
            command: program,
            args: request.args.clone(),
            socket,
        };
        let confinement = Confinement {
            read: c.always_read.iter().cloned().chain(read).collect(),
            scratch,
            written: Vec::new(),
            cwd,
            env: request
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            network,
            spawn: false,
            memory_mb: request.memory_mb,
            open_files: u64::from(request.open_files),
            sockets_group: c.host_group,
        };
        Ok((launch, confinement))
    }

    /// What a dynamic module's `request` reads, each under the stores, and
    /// its working directory, one of them.
    fn bundle(&self, request: &LaunchRequest) -> Result<(Vec<PathBuf>, PathBuf), Status> {
        let read: Vec<PathBuf> = request.read.iter().map(PathBuf::from).collect();
        if let Some(outside) = read
            .iter()
            .find(|r| !clean(r) || !self.config.stores.iter().any(|store| r.starts_with(store)))
        {
            return Err(refused(format!(
                "{} is not under this launcher's stores",
                outside.display()
            )));
        }
        let cwd = PathBuf::from(&request.cwd);
        if !read.contains(&cwd) {
            return Err(refused(format!("{}: not one it reads", request.cwd)));
        }
        Ok((read, cwd))
    }
}

fn refused(message: String) -> Status {
    Status::invalid_argument(message)
}

/// A name that is one path component of letters, digits, `-` and `_`.
fn name(value: &str, what: &str) -> Result<(), Status> {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Ok(());
    }
    Err(refused(format!("{what} {value:?} is not a name")))
}

/// An absolute path that does not climb.
fn clean(path: &Path) -> bool {
    path.is_absolute() && !path.components().any(|c| c == Component::ParentDir)
}

#[tonic::async_trait]
impl launcher_server::Launcher for Served {
    type LaunchStream = ReceiverStream<Result<LaunchEvent, Status>>;

    async fn prepare(
        &self,
        request: Request<PrepareRequest>,
    ) -> Result<Response<PrepareResponse>, Status> {
        let request = request.into_inner();
        let prepared = self
            .preparer
            .prepare(&request.bundle, &request.sha256)
            .await
            .map_err(|e| match e {
                PrepareError::Step(reason) => Status::internal(reason),
                refused => Status::failed_precondition(refused.to_string()),
            })?;
        Ok(Response::new(PrepareResponse {
            bundle: prepared.bundle.to_string_lossy().into_owned(),
            environment: prepared
                .environment
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }))
    }

    async fn launch(
        &self,
        request: Request<LaunchRequest>,
    ) -> Result<Response<Self::LaunchStream>, Status> {
        let (launch, confinement) = self.confinement(request.get_ref())?;
        let confined = Confined::new(
            self.launcher.clone(),
            Arc::clone(&self.watcher),
            confinement,
        );
        let process = confined
            .launch(&launch)
            .await
            .map_err(Status::failed_precondition)?;
        let (events, stream) = mpsc::channel(EVENTS);
        self.followed
            .spawn(follow(process, events, self.stopping.clone()));
        Ok(Response::new(ReceiverStream::new(stream)))
    }
}

/// Send `process`'s events until it ends or its host stops reading, which
/// drops it, so kills it.
async fn follow(
    mut process: Box<dyn ModuleProcess>,
    events: mpsc::Sender<Result<LaunchEvent, Status>>,
    stopping: CancellationToken,
) {
    let event = |event| Ok(LaunchEvent { event: Some(event) });
    if events
        .send(event(Event::Started(LaunchStarted {})))
        .await
        .is_err()
    {
        return;
    }
    let mut lines = process.lines();
    loop {
        tokio::select! {
            () = events.closed() => return,
            () = stopping.cancelled() => {
                // Waited for: a jail's helper removes the jail before the
                // launcher ends, as a pot's stop would kill it and leave
                // mounts only the node can undo.
                if let Err(e) = process.kill().await {
                    tracing::warn!(error = e, "stop a module");
                }
                return;
            }
            line = next(&mut lines) => {
                if let Some(line) = line {
                    if events.send(event(Event::Line(line))).await.is_err() {
                        return;
                    }
                } else {
                    lines = None;
                }
            }
            ended = process.wait() => {
                // What it wrote last comes before how it ended.
                if let Some(lines) = &mut lines {
                    while let Ok(line) = lines.try_recv() {
                        if events.send(event(Event::Line(line))).await.is_err() {
                            return;
                        }
                    }
                }
                let ended = LaunchEnded {
                    how: ended.how,
                    success: ended.success,
                };
                if events.send(event(Event::Ended(ended))).await.is_err() {
                    tracing::debug!("a host left before its module's end");
                }
                return;
            }
        }
    }
}

/// The next line, or forever once there are none.
async fn next(lines: &mut Option<mpsc::Receiver<String>>) -> Option<String> {
    match lines {
        Some(lines) => lines.recv().await,
        None => std::future::pending().await,
    }
}

/// Serve `served` on the Unix socket `socket` until `stop`: its owner's
/// alone, or with its config's `host_group` its hosts' too, who also make
/// their launches' directories in its `sockets`.
///
/// # Errors
/// The socket cannot be bound or given its rights, or the server fails.
pub async fn serve(
    served: Served,
    socket: &Path,
    stop: CancellationToken,
) -> Result<(), std::io::Error> {
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(socket)?;
    match served.config.host_group {
        Some(group) => {
            share(socket, group, 0o660)?;
            share(&served.config.sockets, group, 0o770)?;
        }
        None => std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?,
    }
    let (stopping, followed) = (served.stopping.clone(), served.followed.clone());
    let served = tonic::transport::Server::builder()
        .add_service(LauncherServer::new(served).max_decoding_message_size(MESSAGE_BYTES))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async move {
            stop.cancelled().await;
            stopping.cancel();
        })
        .await
        .map_err(std::io::Error::other);
    followed.close();
    if tokio::time::timeout(MODULES_STOP, followed.wait())
        .await
        .is_err()
    {
        tracing::warn!("modules did not stop in time");
    }
    served
}

#[cfg(test)]
mod tests;
