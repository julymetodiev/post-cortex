// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The sandbox (ARCHITECTURE section 7, sandbox and tools): every file and
//! process access of a module goes through a [`Sandbox`] bound to the
//! module's [`Policy`]. The module in the `sandbox` slot provides it
//! ([`SandboxProvider`]); a module reaches it through its host handle
//! ([`crate::ModuleHost::sandbox`]).

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ModuleError;

mod remote;
mod wire;

pub use remote::RemoteSandbox;

/// `PATH` of a process when the module's configuration names none.
#[cfg(target_os = "macos")]
pub const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin";
/// `PATH` of a process when the module's configuration names none.
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The environment a tool module gives its processes: `PATH` (its
/// configuration's, else [`DEFAULT_PATH`]), `TERM=dumb` and
/// `LANG=C.UTF-8`; the sandbox adds `TMPDIR`, `HOME` and `TMUX_TMPDIR`.
/// A module's own variables come after these ([`OWN_VARIABLES`] are not
/// its to set).
#[must_use]
pub fn process_env(path: Option<&str>) -> Vec<(String, String)> {
    vec![
        ("PATH".to_string(), path.unwrap_or(DEFAULT_PATH).to_string()),
        ("TERM".to_string(), "dumb".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
    ]
}

/// The variables [`process_env`], the sandbox and a call set: a module's
/// configuration does not set them. `GIT_AUTHOR_*` and `GIT_COMMITTER_*`
/// name the agent a call acts for ([`agent_env`]).
pub const OWN_VARIABLES: [&str; 10] = [
    "PATH",
    "TERM",
    "LANG",
    "HOME",
    "TMPDIR",
    "TMUX_TMPDIR",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
];

/// The variables that make a process's commits name `agent` as their
/// author and committer (docs/host.md, a commit's author): git reads them
/// before any `user.name` or `-c user.name`, so a commit through a shell
/// cannot borrow another name, and it needs no identity of the sandbox's
/// (which has none) to commit at all.
#[must_use]
pub fn agent_env(agent: &crate::AgentAuthor) -> Vec<(String, String)> {
    vec![
        ("GIT_AUTHOR_NAME".to_string(), agent.name.clone()),
        ("GIT_AUTHOR_EMAIL".to_string(), agent.email.clone()),
        ("GIT_COMMITTER_NAME".to_string(), agent.name.clone()),
        ("GIT_COMMITTER_EMAIL".to_string(), agent.email.clone()),
    ]
}

/// A directory a module may use, and whether it may write there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// The directory, absolute.
    pub path: PathBuf,
    /// Whether the module may write under it.
    pub write: bool,
}

/// What a module may reach: its roots (its configuration's filesystem
/// entries), the network, new processes, and the names under its roots it
/// never reaches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// Its roots, the first one the default of a relative path.
    pub roots: Vec<Root>,
    /// Whether a process it starts may use the network.
    pub network: bool,
    /// Whether it may start processes.
    pub spawn: bool,
    /// The names (globs: `*.pem`, `.env.*`) of the files and directories
    /// under its roots it never reaches, nor its processes: a path any of
    /// whose components matches is refused (a runner's `[runner] deny`,
    /// docs/connect.md section 7).
    pub deny: Vec<String>,
    /// Whether only this machine's people and agents call it: a client's
    /// call from afar (a gateway's or a tunnel's, which holds it to a
    /// reach) is refused (the tool modules, docs/connect.md section 4).
    pub local_only: bool,
}

/// A path a call names: relative to its root (the first root, or the one
/// it names), or absolute under one of the roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPath {
    /// The root it is relative to; none: the first root.
    pub root: Option<PathBuf>,
    /// The path.
    pub path: PathBuf,
}

impl SandboxPath {
    /// `path` under the first root (or absolute under any).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            root: None,
            path: path.into(),
        }
    }

    /// `path` relative to the root a call names (none: the first root), or
    /// absolute under any.
    pub fn under(path: impl Into<PathBuf>, root: Option<&str>) -> Self {
        Self {
            root: root.map(PathBuf::from),
            path: path.into(),
        }
    }
}

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link (not followed).
    Symlink,
    /// Anything else (a socket, a device).
    Other,
}

/// What a path holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// Its kind.
    pub kind: EntryKind,
    /// Its size in bytes.
    pub size: u64,
    /// When it last changed, if the system says.
    pub modified: Option<SystemTime>,
}

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Its name in the directory.
    pub name: String,
    /// What it holds.
    pub metadata: Metadata,
}

/// A process to run under the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecSpec {
    /// The program (a name looked up on `PATH`, or a path).
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Its working directory; none: the first root.
    pub cwd: Option<SandboxPath>,
    /// Its whole environment: nothing of the host's is inherited.
    pub env: Vec<(String, String)>,
    /// What it reads on stdin; none: nothing.
    pub stdin: Option<Vec<u8>>,
    /// Longest it may run; then its process group is killed.
    pub timeout: Duration,
    /// Most bytes of stdout and of stderr kept; the rest is counted.
    pub output_limit: usize,
}

/// What a running process reports, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEvent {
    /// Bytes it wrote to stdout.
    Stdout(Vec<u8>),
    /// Bytes it wrote to stderr.
    Stderr(Vec<u8>),
    /// It ended; last.
    Exit(ExecExit),
}

/// How a process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecExit {
    /// It exited with this code.
    Code(i32),
    /// A signal ended it.
    Signal,
    /// It ran past its timeout and was killed.
    TimedOut,
    /// Its caller cancelled it and it was killed.
    Cancelled,
}

impl fmt::Display for ExecExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(f, "exit code {code}"),
            Self::Signal => f.write_str("a signal"),
            Self::TimedOut => f.write_str("its timeout"),
            Self::Cancelled => f.write_str("a cancel"),
        }
    }
}

/// A running process: its events until it ends.
pub struct ExecRun {
    events: mpsc::Receiver<ExecEvent>,
}

impl ExecRun {
    /// A run whose events arrive on `events`.
    #[must_use]
    pub fn new(events: mpsc::Receiver<ExecEvent>) -> Self {
        Self { events }
    }

    /// The next event; none after [`ExecEvent::Exit`].
    pub async fn next(&mut self) -> Option<ExecEvent> {
        self.events.recv().await
    }

    /// Wait for the end: the output (each stream at most `limit` bytes,
    /// the last ones kept) and how it ended.
    ///
    /// # Errors
    /// `Unavailable` when the run ended without an exit.
    pub async fn collect(mut self, limit: usize) -> Result<ExecOutput, ModuleError> {
        let (mut stdout, mut stderr, mut truncated) = (Vec::new(), Vec::new(), false);
        while let Some(event) = self.next().await {
            match event {
                ExecEvent::Stdout(bytes) => keep_tail(&mut stdout, &bytes, limit, &mut truncated),
                ExecEvent::Stderr(bytes) => keep_tail(&mut stderr, &bytes, limit, &mut truncated),
                ExecEvent::Exit(exit) => {
                    return Ok(ExecOutput {
                        stdout,
                        stderr,
                        truncated,
                        exit,
                    });
                }
            }
        }
        Err(ModuleError::new(
            crate::ModuleErrorKind::Unavailable,
            "the process ended without an exit status",
        ))
    }
}

/// The output of a finished process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    /// The tail of its stdout.
    pub stdout: Vec<u8>,
    /// The tail of its stderr.
    pub stderr: Vec<u8>,
    /// Whether either was cut to its limit.
    pub truncated: bool,
    /// How it ended.
    pub exit: ExecExit,
}

/// Append `bytes` to `kept`, keeping its last `limit` bytes.
fn keep_tail(kept: &mut Vec<u8>, bytes: &[u8], limit: usize, truncated: &mut bool) {
    kept.extend_from_slice(bytes);
    if kept.len() > limit {
        let cut = kept.len() - limit;
        kept.drain(..cut);
        *truncated = true;
    }
}

/// Files and processes under one module's policy. Every path resolves
/// beneath a root of the policy; anything else is `Denied`.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// The policy this sandbox applies.
    fn policy(&self) -> &Policy;

    /// The bytes of a file, at most `limit`.
    ///
    /// # Errors
    /// `Denied` outside the roots; `NotFound`; `InvalidArgument` for a
    /// directory; `Unavailable` when it cannot be read.
    async fn read(&self, file: &SandboxPath, limit: usize) -> Result<Vec<u8>, ModuleError>;

    /// Replace a file with `bytes` atomically, creating its directories.
    ///
    /// # Errors
    /// `Denied` outside the roots or under a root that does not allow
    /// writes; `Unavailable` when it cannot be written.
    async fn write(&self, file: &SandboxPath, bytes: Vec<u8>) -> Result<(), ModuleError>;

    /// The entries of a directory, by name.
    ///
    /// # Errors
    /// As [`Sandbox::read`].
    async fn list(&self, dir: &SandboxPath) -> Result<Vec<DirEntry>, ModuleError>;

    /// What a path holds (a symbolic link is not followed).
    ///
    /// # Errors
    /// As [`Sandbox::read`].
    async fn metadata(&self, path: &SandboxPath) -> Result<Metadata, ModuleError>;

    /// Remove a file or an empty directory.
    ///
    /// # Errors
    /// As [`Sandbox::write`]; `NotFound`.
    async fn remove(&self, path: &SandboxPath) -> Result<(), ModuleError>;

    /// Move `from` to `to`, both under roots that allow writes.
    ///
    /// # Errors
    /// As [`Sandbox::write`]; `NotFound`.
    async fn rename(&self, from: &SandboxPath, to: &SandboxPath) -> Result<(), ModuleError>;

    /// Start `spec` under the OS sandbox; `cancel` kills it.
    ///
    /// # Errors
    /// `Denied` without the spawn grant or with a working directory
    /// outside the roots; `FailedPrecondition` on a platform without an OS
    /// sandbox; `Unavailable` when it cannot start.
    async fn exec(&self, spec: ExecSpec, cancel: CancellationToken)
    -> Result<ExecRun, ModuleError>;
}

/// What the module in the `sandbox` slot gives the host: a sandbox per
/// module and policy.
pub trait SandboxProvider: Send + Sync {
    /// The sandbox of the module `module` with `policy` (with its scratch
    /// directory when it may start processes).
    ///
    /// # Errors
    /// `Unavailable` when its scratch directory cannot be made.
    fn bind(&self, module: &str, policy: Policy) -> Result<Arc<dyn Sandbox>, ModuleError>;
}

#[cfg(test)]
mod tests;
