// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Starting a module process and keeping it running (T4): a process that
//! exits on its own is started again, with a backoff from
//! [`RESTART_FIRST`] doubling to [`RESTART_MAX`], and started with what
//! the host gave it; a stopped one gets [`EXIT_DEADLINE`] to exit before it
//! is killed.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hyper_util::rt::TokioIo;
use pcx_module_api::{LogRing, LogSource, Manifest};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::module_client::ModuleClient;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Channel, Endpoint, Uri};

use crate::process::ProcessSpec;
use crate::process::launcher::{Launch, ModuleLauncher, ModuleProcess};
use crate::process::link::Link;

/// How long a launched module has to serve its socket.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(10);
/// How long a stopped module has to exit before it is killed.
const EXIT_DEADLINE: Duration = Duration::from_secs(5);
/// Between attempts to reach a launching module.
const CONNECT_RETRY: Duration = Duration::from_millis(20);
/// The wait before the first restart of a module that exited.
const RESTART_FIRST: Duration = Duration::from_millis(500);
/// The longest wait between restarts.
const RESTART_MAX: Duration = Duration::from_secs(60);

/// How long a failed launch's reader gets to keep its last lines.
const OUTPUT_SETTLES: Duration = Duration::from_millis(50);
/// The lines of its output a failed launch's error names.
const LAST_LINES: usize = 5;

/// A launched module process: the process, its channel and client, and
/// its manifest.
pub struct Launched {
    /// The process.
    pub child: Box<dyn ModuleProcess>,
    /// The channel to its socket (it reconnects to a restarted process,
    /// which serves the same socket).
    pub channel: Channel,
    /// Its client.
    pub client: ModuleClient<Channel>,
    /// Its manifest.
    pub manifest: Manifest,
}

/// Launch `spec`'s executable through `launcher`, serving on `socket`, what
/// it writes on stderr kept in `log`, and read its manifest.
///
/// # Errors
/// What went wrong, for the launch error.
pub async fn launch(
    spec: &ProcessSpec,
    socket: &Path,
    log: &Arc<LogRing>,
    launcher: &dyn ModuleLauncher,
) -> Result<Launched, String> {
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("stale socket {}: {e}", socket.display())),
    }
    let mut child = launcher
        .launch(&Launch {
            id: spec.id.clone(),
            command: spec.command.clone(),
            args: spec.args.clone(),
            socket: socket.to_path_buf(),
        })
        .await?;
    if let Some(lines) = child.lines() {
        tokio::spawn(keep(lines, Arc::clone(log), spec.id.clone()));
    }
    let channel = connect(socket, child.as_mut()).await?;
    let mut client = ModuleClient::new(channel.clone());
    let manifest = client
        .get_manifest(wire::GetManifestRequest {})
        .await
        .map_err(|s| format!("GetManifest: {}", s.message()))?
        .into_inner();
    Ok(Launched {
        child,
        channel,
        client,
        manifest,
    })
}

/// Keep each line the process writes in `log`, and in the host's own log,
/// until it closes.
async fn keep(mut lines: mpsc::Receiver<String>, log: Arc<LogRing>, id: String) {
    while let Some(line) = lines.recv().await {
        tracing::info!(module = id, line, "module output");
        log.push(LogSource::Process, line);
    }
}

/// Reach the module's socket, while its process runs, within the launch
/// deadline.
async fn connect(socket: &Path, child: &mut dyn ModuleProcess) -> Result<Channel, String> {
    let started = Instant::now();
    loop {
        if let Some(ended) = child.ended()? {
            return Err(format!("{} before serving its socket", ended.how));
        }
        if socket.exists() {
            let path = socket.to_path_buf();
            // The URI is not used: every connection is the socket.
            let endpoint = Endpoint::from_static("http://module.sock");
            let channel = endpoint
                .connect_with_connector(tower::service_fn(move |_: Uri| {
                    let path = path.clone();
                    async move { UnixStream::connect(path).await.map(TokioIo::new) }
                }))
                .await;
            if let Ok(channel) = channel {
                return Ok(channel);
            }
        }
        if started.elapsed() > LAUNCH_DEADLINE {
            return Err(format!(
                "did not serve {} within {LAUNCH_DEADLINE:?}",
                socket.display()
            ));
        }
        tokio::time::sleep(CONNECT_RETRY).await;
    }
}

/// What a supervisor keeps running.
pub struct Supervised {
    /// The module's spec.
    pub spec: ProcessSpec,
    /// Its socket.
    pub socket: std::path::PathBuf,
    /// The socket of its callbacks.
    pub host_socket: std::path::PathBuf,
    /// Its link, updated at each restart.
    pub link: Arc<Link>,
    /// Whether it subscribed to events.
    pub events: bool,
    /// Cancelled when the host stops it.
    pub stopping: CancellationToken,
    /// Its log.
    pub log: Arc<LogRing>,
    /// What starts it again.
    pub launcher: Arc<dyn ModuleLauncher>,
}

/// Keep `child` running until the module is stopped, then give it the exit
/// deadline.
pub async fn supervise(mut child: Box<dyn ModuleProcess>, module: Supervised) {
    let id = module.spec.id.clone();
    loop {
        tokio::select! {
            biased;
            () = module.stopping.cancelled() => {
                exit(child, &id).await;
                return;
            }
            ended = child.wait() => {
                tracing::warn!(module = id, ended = ended.how, "module process ended");
                module.log.push(LogSource::Host, format!("the process {}", ended.how));
            }
        }
        module.link.close_events();
        let mut delay = RESTART_FIRST;
        child = loop {
            tokio::select! {
                () = module.stopping.cancelled() => return,
                () = tokio::time::sleep(delay) => {}
            }
            match restart(&module).await {
                Ok(child) => break child,
                Err(e) => {
                    tracing::warn!(
                        module = id,
                        error = e,
                        "module restart failed; retried in {delay:?}"
                    );
                    module.log.push(
                        LogSource::Host,
                        format!("it did not start again: {e}; tried again in {delay:?}"),
                    );
                    delay = (delay * 2).min(RESTART_MAX);
                }
            }
        };
        tracing::info!(module = id, "module process restarted");
        module
            .log
            .push(LogSource::Host, "the process started again");
    }
}

/// Launch the module again and start it as the host started it.
async fn restart(module: &Supervised) -> Result<Box<dyn ModuleProcess>, String> {
    let launched = launch(
        &module.spec,
        &module.socket,
        &module.log,
        module.launcher.as_ref(),
    )
    .await?;
    if launched.manifest.id != module.spec.id {
        return Err(format!("the process names itself {}", launched.manifest.id));
    }
    let ctx = module
        .link
        .started_with()
        .ok_or_else(|| "the module was never started".to_string())?;
    module
        .link
        .start(
            launched.client,
            ctx,
            module.events,
            &module.spec.id,
            &module.host_socket,
        )
        .await
        .map_err(|e| e.message)?;
    Ok(launched.child)
}

/// Wait for a stopped module's process to exit; kill it past the deadline.
pub async fn exit(mut child: Box<dyn ModuleProcess>, id: &str) {
    if tokio::time::timeout(EXIT_DEADLINE, child.wait())
        .await
        .is_err()
    {
        tracing::warn!(module = id, "the module did not exit; killed");
        if let Err(e) = child.kill().await {
            tracing::warn!(module = id, error = %e, "kill the module");
        }
    }
}

/// `reason` a launch failed, with the last of what the process wrote, the
/// reader given a moment to keep it: usually what says why.
pub async fn with_output(reason: String, log: &LogRing) -> String {
    tokio::time::sleep(OUTPUT_SETTLES).await;
    let lines = log.lines();
    let last: Vec<&str> = lines
        .iter()
        .rev()
        .take(LAST_LINES)
        .rev()
        .map(|line| line.text.as_str())
        .collect();
    if last.is_empty() {
        reason
    } else {
        format!("{reason}: {}", last.join(" | "))
    }
}

/// Remove a launch's directory of sockets, if it is there.
pub fn remove_sockets(dir: &Path, id: &str) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(module = id, error = %e, "remove the sockets"),
    }
}
