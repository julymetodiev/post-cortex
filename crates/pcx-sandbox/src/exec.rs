// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Running a process: the platform's command around the program (the OS
//! sandbox), its own process group, its whole environment from the call,
//! its output streamed as it comes, and its group killed past its timeout
//! or on a cancel.

use std::path::PathBuf;
use std::process::Stdio;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use pcx_module_api::sandbox::{ExecEvent, ExecExit, ExecRun, ExecSpec};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Events of a run buffered before its output waits.
const EVENT_QUEUE: usize = 64;

/// Bytes read from a stream at a time.
const CHUNK: usize = 8 * 1024;

/// A command line: the program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    /// The program run.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
}

/// Start `command` (the program of `spec` inside the OS sandbox) in `cwd`
/// with `spec`'s environment and stdin; its events until it ends.
///
/// # Errors
/// `Unavailable` when it does not start.
pub fn start(
    spec: ExecSpec,
    command: CommandLine,
    cwd: PathBuf,
    cancel: CancellationToken,
) -> Result<ExecRun, ModuleError> {
    let mut cmd = Command::new(&command.program);
    cmd.args(&command.args)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(&cwd)
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| {
        ModuleError::new(
            ModuleErrorKind::Unavailable,
            format!("{} did not start: {e}", spec.program),
        )
    })?;
    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    let stdin = child.stdin.take().zip(spec.stdin);
    let readers = [
        child
            .stdout
            .take()
            .map(|out| tokio::spawn(forward(out, tx.clone(), ExecEvent::Stdout))),
        child
            .stderr
            .take()
            .map(|err| tokio::spawn(forward(err, tx.clone(), ExecEvent::Stderr))),
    ];
    tokio::spawn(async move {
        if let Some((mut pipe, bytes)) = stdin {
            // A process that stops reading early closes the pipe: not an error.
            if let Err(e) = pipe.write_all(&bytes).await {
                tracing::debug!(error = %e, "stdin of a sandboxed process closed early");
            }
        }
        let exit = wait(&mut child, spec.timeout, &cancel).await;
        for reader in readers.into_iter().flatten() {
            if let Err(e) = reader.await {
                tracing::warn!(error = %e, "output reader of a sandboxed process");
            }
        }
        if tx.send(ExecEvent::Exit(exit)).await.is_err() {
            tracing::debug!("a sandboxed run's reader left before its exit");
        }
    });
    Ok(ExecRun::new(rx))
}

/// Forward `stream` to `tx` in chunks until it ends or the reader leaves.
async fn forward(
    mut stream: impl AsyncRead + Unpin,
    tx: mpsc::Sender<ExecEvent>,
    event: fn(Vec<u8>) -> ExecEvent,
) {
    let mut buf = vec![0u8; CHUNK];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => return,
            Ok(n) => {
                if tx.send(event(buf[..n].to_vec())).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "output of a sandboxed process");
                return;
            }
        }
    }
}

/// How `child` ends: by itself, killed past `timeout`, or killed on a
/// cancel (its whole process group).
async fn wait(
    child: &mut Child,
    timeout: std::time::Duration,
    cancel: &CancellationToken,
) -> ExecExit {
    let ended = tokio::select! {
        status = child.wait() => return exit_of(status),
        () = tokio::time::sleep(timeout) => ExecExit::TimedOut,
        () = cancel.cancelled() => ExecExit::Cancelled,
    };
    kill_group(child);
    if let Err(e) = child.wait().await {
        tracing::warn!(error = %e, "wait for a killed sandboxed process");
    }
    ended
}

fn exit_of(status: std::io::Result<std::process::ExitStatus>) -> ExecExit {
    match status {
        Ok(status) => status.code().map_or(ExecExit::Signal, ExecExit::Code),
        Err(e) => {
            tracing::warn!(error = %e, "wait for a sandboxed process");
            ExecExit::Signal
        }
    }
}

/// Kill the process group `child` leads (it was started as its leader).
fn kill_group(child: &Child) {
    let Some(pid) = child.id().and_then(|id| i32::try_from(id).ok()) else {
        return;
    };
    if let Err(e) = killpg(Pid::from_raw(pid), Signal::SIGKILL) {
        tracing::debug!(error = %e, "kill the process group of a sandboxed process");
    }
}
