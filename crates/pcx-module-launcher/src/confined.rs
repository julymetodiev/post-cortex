// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's process under the OS sandbox, with its limits, its roots read
//! only but for its sockets' directory and its emptied scratch directory.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use pcx_host::{Ended, Launch, ModuleLauncher, ModuleProcess, output_lines};
use pcx_module_api::SOCKET_ENV;
use pcx_module_api::sandbox::{Policy, Root};
use pcx_sandbox::{Deny, Launcher, Limits};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::watch::{HELPER_WAITS, Watcher};

/// The lines of a failed program's output its error names.
const TAIL: usize = 12;

/// What a module's process may reach and use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confinement {
    /// What it reads.
    pub read: Vec<PathBuf>,
    /// What it writes, besides its scratch directory and its sockets.
    pub written: Vec<PathBuf>,
    /// Its scratch directory, emptied at each launch, its `TMPDIR` and
    /// `HOME`.
    pub scratch: PathBuf,
    /// Its working directory.
    pub cwd: PathBuf,
    /// Its whole environment, but for its socket, `TMPDIR` and `HOME`.
    pub env: Vec<(String, String)>,
    /// Whether it may use the network.
    pub network: bool,
    /// Whether it may start processes.
    pub spawn: bool,
    /// Its memory limit, in MB.
    pub memory_mb: u32,
    /// Its limit of open files.
    pub open_files: u64,
    /// The group that reaches its sockets besides its user: its host's,
    /// when the host does not run as root ([`crate::ServedConfig`]'s
    /// `host_group`); none: its user's alone.
    pub sockets_group: Option<u32>,
}

/// `path` given `group` and `mode`, so a host of that group reaches what is
/// there (docs/modules/dynamic.md, the host's sockets).
///
/// # Errors
/// Its group or its mode cannot change.
pub fn share(path: &Path, group: u32, mode: u32) -> std::io::Result<()> {
    std::os::unix::fs::chown(path, None, Some(group))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Starts a module's process confined.
pub struct Confined {
    launcher: Launcher,
    watcher: Arc<Watcher>,
    confinement: Confinement,
}

impl Confined {
    /// Processes under `confinement`, confined by `launcher`, their memory
    /// watched by `watcher`.
    #[must_use]
    pub fn new(launcher: Launcher, watcher: Arc<Watcher>, confinement: Confinement) -> Self {
        Self {
            launcher,
            watcher,
            confinement,
        }
    }

    /// The scratch directory, empty and its process's.
    fn scratch(&self) -> Result<(), String> {
        let scratch = &self.confinement.scratch;
        let named = |e: std::io::Error| format!("scratch {}: {e}", scratch.display());
        match std::fs::remove_dir_all(scratch) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(named(e)),
        }
        std::fs::create_dir_all(scratch).map_err(named)?;
        self.launcher.give(scratch).map_err(named)
    }
}

impl Confined {
    /// `program args` started under the confinement, also writing `written`
    /// and with `env` beside its own; `output`: its stdout is read too.
    fn start(
        &self,
        program: &Path,
        args: &[String],
        written: &[PathBuf],
        env: &[(&str, &Path)],
        output: bool,
    ) -> Result<ConfinedProcess, String> {
        let c = &self.confinement;
        self.scratch()?;
        let read = c.read.iter().map(|path| Root {
            path: path.clone(),
            write: false,
        });
        // The scratch directory last: it makes the module's jail its own.
        let written = written
            .iter()
            .chain(&c.written)
            .cloned()
            .chain([c.scratch.clone()])
            .map(|path| Root { path, write: true });
        let policy = Policy {
            roots: read.chain(written).collect(),
            network: c.network,
            spawn: c.spawn,
            deny: Vec::new(),
            local_only: false,
        };
        let deny = Deny::new(&[]).map_err(|e| e.to_string())?;
        let limits = Limits {
            open_files: c.open_files,
            no_processes: !c.spawn,
        };
        let line = self
            .launcher
            .confine_module(&policy, &deny, &limits, &program.to_string_lossy(), args)
            .map_err(|e| e.message)?;
        let mut child = Command::new(&line.program)
            .args(&line.args)
            .env_clear()
            .envs(c.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .envs(env.iter().copied())
            .env("TMPDIR", &c.scratch)
            .env("HOME", &c.scratch)
            .current_dir(&c.cwd)
            // Closed, a helper that waits kills the process and removes its
            // jail, however this host ends; a killed helper could not.
            .stdin(if HELPER_WAITS {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            // A module's stdout is nobody's: unread, a pipe would stop it.
            .stdout(if output {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(!HELPER_WAITS)
            .spawn()
            .map_err(|e| format!("start {}: {e}", line.program.display()))?;
        let leader = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .ok_or_else(|| format!("{} ended as it started", program.display()))?;
        let over = self.watcher.watch(leader, c.memory_mb);
        Ok(ConfinedProcess {
            stdin: child.stdin.take(),
            child,
            leader,
            over,
            memory_mb: c.memory_mb,
            watcher: Arc::clone(&self.watcher),
        })
    }

    /// `program args` run to its end within `deadline`, past which it is
    /// killed.
    ///
    /// # Errors
    /// It does not start.
    pub async fn run(
        &self,
        program: &Path,
        args: &[String],
        deadline: Duration,
    ) -> Result<Ran, String> {
        let mut process = self.start(program, args, &[], &[], true)?;
        let mut streams = Vec::new();
        streams.extend(process.child.stdout.take().map(output_lines));
        streams.extend(process.child.stderr.take().map(output_lines));
        let mut collected = Vec::new();
        for mut stream in streams {
            collected.push(tokio::spawn(async move {
                let mut lines = Vec::new();
                while let Some(line) = stream.recv().await {
                    lines.push(line);
                }
                lines
            }));
        }
        let ended = tokio::time::timeout(deadline, process.child.wait()).await;
        let end = match ended {
            Ok(Ok(status)) => process.how(status),
            Ok(Err(e)) => Ended {
                how: format!("was lost: {e}"),
                success: false,
            },
            Err(_) => {
                process.kill().await?;
                Ended {
                    how: format!("did not end within {deadline:?}"),
                    success: false,
                }
            }
        };
        let mut lines = Vec::new();
        for reader in collected {
            lines.extend(reader.await.map_err(|e| e.to_string())?);
        }
        if !end.success {
            lines.push(format!("{} {}", program.display(), end.how));
        }
        Ok(Ran {
            success: end.success,
            lines,
        })
    }
}

/// How a program run to its end went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Whether it succeeded.
    pub success: bool,
    /// What it wrote, and how it ended when it failed.
    pub lines: Vec<String>,
}

impl Ran {
    /// The last of what it wrote, which says why.
    #[must_use]
    pub fn tail(&self) -> String {
        let from = self.lines.len().saturating_sub(TAIL);
        self.lines[from..].join(" | ")
    }
}

#[async_trait]
impl ModuleLauncher for Confined {
    async fn launch(&self, launch: &Launch) -> Result<Box<dyn ModuleProcess>, String> {
        let sockets = launch
            .socket
            .parent()
            .ok_or_else(|| format!("{}: a socket has a directory", launch.socket.display()))?;
        let named = |e: std::io::Error| format!("sockets {}: {e}", sockets.display());
        self.launcher.give(sockets).map_err(named)?;
        // Before the process starts, so the socket it makes takes the
        // directory's group.
        if let Some(group) = self.confinement.sockets_group {
            share(sockets, group, 0o770).map_err(named)?;
        }
        let process = self.start(
            &launch.command,
            &launch.args,
            &[sockets.to_path_buf()],
            &[(SOCKET_ENV, launch.socket.as_path())],
            false,
        )?;
        Ok(Box::new(process))
    }
}

struct ConfinedProcess {
    child: Child,
    /// What a helper that waits watches.
    stdin: Option<ChildStdin>,
    leader: i32,
    over: Arc<AtomicBool>,
    memory_mb: u32,
    watcher: Arc<Watcher>,
}

impl ConfinedProcess {
    fn how(&self, status: std::process::ExitStatus) -> Ended {
        if self.over.load(Ordering::SeqCst) {
            Ended {
                how: format!("was killed past its memory limit of {} MB", self.memory_mb),
                success: false,
            }
        } else {
            Ended {
                how: format!("exited ({status})"),
                success: status.success(),
            }
        }
    }

    fn stop(&mut self) -> Result<(), String> {
        if HELPER_WAITS {
            self.stdin = None;
            return Ok(());
        }
        match killpg(Pid::from_raw(self.leader), Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

impl Drop for ConfinedProcess {
    fn drop(&mut self) {
        self.watcher.forget(self.leader);
        if let Err(e) = self.stop() {
            tracing::warn!(error = e, "kill a dropped module's process group");
        }
    }
}

#[async_trait]
impl ModuleProcess for ConfinedProcess {
    fn lines(&mut self) -> Option<mpsc::Receiver<String>> {
        self.child.stderr.take().map(output_lines)
    }

    fn ended(&mut self) -> Result<Option<Ended>, String> {
        let status = self.child.try_wait().map_err(|e| format!("wait: {e}"))?;
        Ok(status.map(|s| self.how(s)))
    }

    async fn wait(&mut self) -> Ended {
        match self.child.wait().await {
            Ok(status) => self.how(status),
            Err(e) => Ended {
                how: format!("was lost: {e}"),
                success: false,
            },
        }
    }

    async fn kill(&mut self) -> Result<(), String> {
        self.stop()?;
        self.child.wait().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}
