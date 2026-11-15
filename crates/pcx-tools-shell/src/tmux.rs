// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! tmux sessions of `tools-shell`: long-running commands in the module's
//! own tmux server (`tmux -L pcx`), which runs under the module's sandbox
//! with its socket in the scratch directory (`TMUX_TMPDIR`). tmux is the
//! one record of the sessions; the service keeps none.

use std::time::Duration;

use pcx_module_api::sandbox::{ExecExit, SandboxPath};
use pcx_module_api::{ModuleError, ModuleErrorKind};

use crate::shell::{BASH, Ran, Shell};

/// The tmux program.
const TMUX: &str = "tmux";

/// The shell a pane starts its command with (the command itself runs in
/// bash): the one every Unix has at this path.
const DEFAULT_SHELL: &str = "/bin/sh";

/// The module's tmux server (`-L`), apart from the user's.
const SERVER: &str = "pcx";

/// Longest a tmux command may take.
const TMUX_TIMEOUT: Duration = Duration::from_secs(10);

/// Lines of a pane read when the call names none.
const DEFAULT_READ_LINES: usize = 100;

/// Lines of a pane read at most.
const MAX_READ_LINES: usize = 2000;

/// A session of the module's tmux server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Its name, as tmux gave it.
    pub name: String,
    /// The command it runs.
    pub command: String,
    /// Whether that command has ended (its pane stays to be read).
    pub exited: bool,
}

/// The tmux sessions of one module, through its sandbox.
pub struct Tmux<'a> {
    shell: &'a Shell<'a>,
}

impl<'a> Tmux<'a> {
    /// The sessions `shell`'s processes keep.
    pub fn new(shell: &'a Shell<'a>) -> Self {
        Self { shell }
    }

    /// A new session running `command` in `cwd` (none: the first root);
    /// its pane stays after the command ends, to be read.
    ///
    /// # Errors
    /// As the sandbox's exec; `Unavailable` when tmux refuses.
    pub async fn start(
        &self,
        command: &str,
        cwd: Option<SandboxPath>,
    ) -> Result<String, ModuleError> {
        // The session gets the agent's variables itself: a running server
        // gives a new session its own environment, not the caller's.
        let agent: Vec<String> = self
            .shell
            .agent()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        let mut args = vec![
            "start-server",
            ";",
            "set-option",
            "-wg",
            "remain-on-exit",
            "on",
            ";",
            // A pane runs through the default shell, which is the
            // user's from the password file when the environment
            // names none: a jail's user has nologin.
            "set-option",
            "-g",
            "default-shell",
            DEFAULT_SHELL,
            ";",
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_name}",
        ];
        for variable in &agent {
            args.extend(["-e", variable.as_str()]);
        }
        args.extend(["--", BASH, "-c", command]);
        let ran = self.tmux(&args, cwd).await?;
        // tmux answers 0 even when its server could not start: the
        // session it names must exist.
        let name = ran.output.trim().to_string();
        let started = self.run(&["has-session", "-t", &name], None).await?;
        match started.exit {
            ExecExit::Code(0) if !name.is_empty() => Ok(name),
            _ => Err(ModuleError::new(
                ModuleErrorKind::Unavailable,
                format!("tmux did not start the session: {name}"),
            )),
        }
    }

    /// The last `lines` lines of session `name`'s pane (1 to 2000; none:
    /// 100), and whether its command has ended.
    ///
    /// # Errors
    /// `InvalidArgument` for a count out of range; `NotFound` for no such
    /// session; as the sandbox's exec.
    pub async fn read(
        &self,
        name: &str,
        lines: Option<usize>,
    ) -> Result<(String, bool), ModuleError> {
        let lines = lines.unwrap_or(DEFAULT_READ_LINES);
        if lines == 0 || lines > MAX_READ_LINES {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("lines is 1 to {MAX_READ_LINES}, not {lines}"),
            ));
        }
        self.require(name).await?;
        let from = format!("-{lines}");
        let pane = self
            .tmux(&["capture-pane", "-p", "-t", name, "-S", &from], None)
            .await?;
        let dead = self
            .tmux(&["display-message", "-p", "-t", name, "#{pane_dead}"], None)
            .await?;
        Ok((
            pane.output.trim_end().to_string(),
            dead.output.trim() == "1",
        ))
    }

    /// Type `text` into session `name`, then Enter when `enter`.
    ///
    /// # Errors
    /// `NotFound` for no such session; as the sandbox's exec.
    pub async fn send(&self, name: &str, text: &str, enter: bool) -> Result<(), ModuleError> {
        self.require(name).await?;
        self.tmux(&["send-keys", "-t", name, "-l", text], None)
            .await?;
        if enter {
            self.tmux(&["send-keys", "-t", name, "Enter"], None).await?;
        }
        Ok(())
    }

    /// The module's sessions.
    ///
    /// # Errors
    /// As the sandbox's exec.
    pub async fn list(&self) -> Result<Vec<Session>, ModuleError> {
        let listed = self
            .tmux(
                &[
                    "start-server",
                    ";",
                    "list-panes",
                    "-a",
                    "-F",
                    "#{session_name}\t#{pane_dead}\t#{pane_start_command}",
                ],
                None,
            )
            .await?;
        Ok(listed
            .output
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let (name, dead, command) = (parts.next()?, parts.next()?, parts.next()?);
                Some(Session {
                    name: name.to_string(),
                    command: command.to_string(),
                    exited: dead == "1",
                })
            })
            .collect())
    }

    /// End session `name` and its command.
    ///
    /// # Errors
    /// `NotFound` for no such session; as the sandbox's exec.
    pub async fn kill(&self, name: &str) -> Result<(), ModuleError> {
        self.require(name).await?;
        self.tmux(&["kill-session", "-t", name], None).await?;
        Ok(())
    }

    /// End the module's tmux server and every session in it; nothing to do
    /// when none runs.
    ///
    /// # Errors
    /// As the sandbox's exec.
    pub async fn stop(&self) -> Result<(), ModuleError> {
        self.run(&["start-server", ";", "kill-server"], None)
            .await?;
        Ok(())
    }

    /// `NotFound` unless session `name` exists (`has-session` answers 1).
    async fn require(&self, name: &str) -> Result<(), ModuleError> {
        let ran = self.run(&["has-session", "-t", name], None).await?;
        match ran.exit {
            ExecExit::Code(0) => Ok(()),
            ExecExit::Code(1) => Err(ModuleError::new(
                ModuleErrorKind::NotFound,
                format!("no tmux session {name}; tmux_list names the running ones"),
            )),
            exit => Err(refused(&ran, exit)),
        }
    }

    /// Run tmux with `args`; `Unavailable` unless it exits 0.
    async fn tmux(&self, args: &[&str], cwd: Option<SandboxPath>) -> Result<Ran, ModuleError> {
        let ran = self.run(args, cwd).await?;
        match ran.exit {
            ExecExit::Code(0) => Ok(ran),
            exit => Err(refused(&ran, exit)),
        }
    }

    /// Run tmux with `args` on the module's server.
    async fn run(&self, args: &[&str], cwd: Option<SandboxPath>) -> Result<Ran, ModuleError> {
        let args = ["-L", SERVER]
            .iter()
            .chain(args)
            .map(|a| (*a).to_string())
            .collect();
        self.shell.run(TMUX, args, cwd, TMUX_TIMEOUT).await
    }
}

/// tmux's refusal, with what it said.
fn refused(ran: &Ran, exit: ExecExit) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::Unavailable,
        format!("tmux ended with {exit}: {}", ran.output.trim()),
    )
}
