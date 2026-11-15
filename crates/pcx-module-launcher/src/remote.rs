// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The host's side of the launcher of the pot `pcx-modules`: a module's
//! process started there, followed over its socket. Dropped, the call ends,
//! and the launcher kills the process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use hyper_util::rt::TokioIo;
use pcx_host::{Ended, Launch, ModuleLauncher, ModuleProcess};
use post_cortex_proto::module::launch_event::Event;
use post_cortex_proto::module::launcher_client::LauncherClient;
use post_cortex_proto::module::{LaunchEvent, LaunchRequest, PrepareRequest, PrepareResponse};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tonic::Streaming;
use tonic::transport::{Channel, Endpoint, Uri};

use crate::served::MESSAGE_BYTES;

/// Lines of a process buffered before its reader waits.
const LINES: usize = 256;

/// What a launch asks of the launcher, beside what to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requested {
    /// Whose module it is.
    pub owner: String,
    /// What it reads.
    pub read: Vec<PathBuf>,
    /// Its working directory, one of `read`.
    pub cwd: PathBuf,
    /// Its environment.
    pub env: HashMap<String, String>,
    /// Its memory limit, in MB.
    pub memory_mb: u32,
    /// Its limit of open files.
    pub open_files: u32,
    /// A program's approved SHA-256, which the launcher checks; empty for a
    /// runtime (the launcher's image's).
    pub sha256: String,
}

/// Starts a module's process through the launcher at a socket.
#[derive(Debug, Clone)]
pub struct Remote {
    socket: PathBuf,
    requested: Requested,
}

impl Remote {
    /// `requested`, of the launcher at `socket`.
    #[must_use]
    pub fn new(socket: PathBuf, requested: Requested) -> Self {
        Self { socket, requested }
    }
}

/// The launcher at `socket`.
async fn client(socket: &Path) -> Result<LauncherClient<Channel>, String> {
    let path = socket.to_path_buf();
    // The URI is not used: every connection is the socket.
    let channel = Endpoint::from_static("http://launcher.sock")
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let path = path.clone();
            async move { UnixStream::connect(path).await.map(TokioIo::new) }
        }))
        .await
        .map_err(|e| format!("the launcher at {}: {e}", socket.display()))?;
    Ok(LauncherClient::new(channel).max_encoding_message_size(MESSAGE_BYTES))
}

/// The bundle `tar` of hash `sha256`, prepared by the launcher at `socket`.
///
/// # Errors
/// It is refused or fails; the message says why.
pub async fn prepare(socket: &Path, tar: Vec<u8>, sha256: &str) -> Result<PrepareResponse, String> {
    client(socket)
        .await?
        .prepare(PrepareRequest {
            bundle: tar,
            sha256: sha256.to_string(),
        })
        .await
        .map(tonic::Response::into_inner)
        .map_err(|s| s.message().to_string())
}

#[async_trait]
impl ModuleLauncher for Remote {
    /// The launcher runs what its image holds, out of this host's reach.
    fn pins(&self) -> bool {
        false
    }

    async fn launch(&self, launch: &Launch) -> Result<Box<dyn ModuleProcess>, String> {
        let r = &self.requested;
        let mut events = client(&self.socket)
            .await?
            .launch(LaunchRequest {
                module: launch.id.clone(),
                owner: r.owner.clone(),
                program: launch.command.to_string_lossy().into_owned(),
                args: launch.args.clone(),
                env: r.env.clone(),
                socket: launch.socket.to_string_lossy().into_owned(),
                read: r
                    .read
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
                cwd: r.cwd.to_string_lossy().into_owned(),
                memory_mb: r.memory_mb,
                open_files: r.open_files,
                sha256: r.sha256.clone(),
            })
            .await
            .map_err(|s| format!("the launcher refused: {}", s.message()))?
            .into_inner();
        match events.message().await {
            Ok(Some(LaunchEvent {
                event: Some(Event::Started(_)),
            })) => {}
            Ok(other) => return Err(format!("the launcher answered {other:?}, not `started`")),
            Err(s) => return Err(format!("the launcher: {}", s.message())),
        }
        let (lines, read) = mpsc::channel(LINES);
        let (ended, ended_read) = watch::channel(None);
        let pump = tokio::spawn(follow(events, lines, ended));
        Ok(Box::new(RemoteProcess {
            lines: Some(read),
            ended: ended_read,
            pump,
        }))
    }
}

/// Hand on the process's events until the last.
async fn follow(
    mut events: Streaming<LaunchEvent>,
    lines: mpsc::Sender<String>,
    ended: watch::Sender<Option<Ended>>,
) {
    let how = loop {
        match events.message().await {
            Ok(Some(LaunchEvent { event })) => match event {
                Some(Event::Line(line)) => {
                    if lines.send(line).await.is_err() {
                        tracing::debug!("a module's log left before it");
                    }
                }
                Some(Event::Ended(how)) => {
                    break Ended {
                        how: how.how,
                        success: how.success,
                    };
                }
                Some(Event::Started(_)) | None => {}
            },
            Ok(None) => {
                break Ended {
                    how: "was lost: the launcher ended its call".to_string(),
                    success: false,
                };
            }
            Err(s) => {
                break Ended {
                    how: format!("was lost: the launcher: {}", s.message()),
                    success: false,
                };
            }
        }
    };
    ended.send_replace(Some(how));
}

struct RemoteProcess {
    lines: Option<mpsc::Receiver<String>>,
    ended: watch::Receiver<Option<Ended>>,
    pump: JoinHandle<()>,
}

impl Drop for RemoteProcess {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

#[async_trait]
impl ModuleProcess for RemoteProcess {
    fn lines(&mut self) -> Option<mpsc::Receiver<String>> {
        self.lines.take()
    }

    fn ended(&mut self) -> Result<Option<Ended>, String> {
        Ok(self.ended.borrow().clone())
    }

    async fn wait(&mut self) -> Ended {
        let lost = |how: &str| Ended {
            how: how.to_string(),
            success: false,
        };
        match self.ended.wait_for(Option::is_some).await {
            Ok(how) => how
                .clone()
                .unwrap_or_else(|| lost("was lost: its follower said nothing")),
            Err(_) => lost("was lost: its follower ended"),
        }
    }

    async fn kill(&mut self) -> Result<(), String> {
        self.pump.abort();
        Ok(())
    }
}
