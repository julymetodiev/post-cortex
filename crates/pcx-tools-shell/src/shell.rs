// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The process service of `tools-shell`: typed requests over the module's
//! sandbox, between the tool handlers (JSON) and the `Sandbox` port.

use std::time::Duration;

use pcx_module_api::AgentAuthor;
use pcx_module_api::sandbox::{ExecExit, ExecSpec, Sandbox, SandboxPath, agent_env, process_env};
use pcx_module_api::text::tail;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use tokio_util::sync::CancellationToken;

/// The shell a command (and a tmux session's) runs in, found through the
/// module's `path` as every program it runs is: `/bin/bash` on Linux and
/// macOS, `/usr/local/bin/bash` on FreeBSD.
pub const BASH: &str = "bash";

/// Seconds a command runs when the call names none.
const DEFAULT_TIMEOUT: u64 = 120;

/// Seconds a command may be given at most.
const MAX_TIMEOUT: u64 = 600;

/// Bytes of stdout and of stderr a run keeps (its tail).
pub const OUTPUT_LIMIT: usize = 1024 * 1024;

/// Lines of a bash command's output answered at most (its end).
const ANSWER_LINES: usize = 2000;

/// Bytes of a bash command's output answered at most (its end).
const ANSWER_BYTES: usize = 50 * 1024;

/// A bash command run to its end, as it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// The end of what it printed: stdout, then stderr.
    pub output: String,
    /// Its lines in all.
    pub lines: usize,
    /// How it ended.
    pub exit: ExecExit,
    /// Whether output is only its end.
    pub truncated: bool,
}

/// A program run to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// What it printed: stdout, then stderr.
    pub output: String,
    /// How it ended.
    pub exit: ExecExit,
    /// Whether the start of its output was cut.
    pub truncated: bool,
}

/// The processes of one module, through its sandbox.
pub struct Shell<'a> {
    sandbox: &'a dyn Sandbox,
    /// `PATH` of its processes; none: the module API's default.
    path: Option<&'a str>,
    /// Their other variables.
    env: &'a [(String, String)],
    /// The variables that name the agent a call acts for as its commits'
    /// author; none for a person's call.
    agent: Vec<(String, String)>,
}

impl<'a> Shell<'a> {
    /// The processes `sandbox` lets the module start, with `path` as their
    /// `PATH` and `env` besides, for a call by `agent` (none: a person's or no
    /// one's), whose commits name it.
    pub fn new(
        sandbox: &'a dyn Sandbox,
        path: Option<&'a str>,
        env: &'a [(String, String)],
        agent: Option<&AgentAuthor>,
    ) -> Self {
        Self {
            sandbox,
            path,
            env,
            agent: agent.map(agent_env).unwrap_or_default(),
        }
    }

    /// The variables that name the agent its call acts for; empty for a
    /// person's call.
    pub fn agent(&self) -> &[(String, String)] {
        &self.agent
    }

    /// `command` run by bash in `cwd` (none: the first root), killed past
    /// `timeout` seconds (1 to 600; none: 120): the end of its output (2000
    /// lines, 50 KiB).
    ///
    /// # Errors
    /// `InvalidArgument` for a timeout out of range; as the sandbox's exec.
    pub async fn bash(
        &self,
        command: &str,
        cwd: Option<SandboxPath>,
        timeout: Option<u64>,
    ) -> Result<Answered, ModuleError> {
        let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
        if timeout == 0 || timeout > MAX_TIMEOUT {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!(
                    "timeout is 1 to {MAX_TIMEOUT} seconds, not {timeout}; tmux_start runs a longer command"
                ),
            ));
        }
        let ran = self
            .run(
                BASH,
                // A pipe answers how its worst command ended: a failed test
                // piped through `tail` is not `exit 0`.
                vec![
                    "-o".to_string(),
                    "pipefail".to_string(),
                    "-c".to_string(),
                    command.to_string(),
                ],
                cwd,
                Duration::from_secs(timeout),
            )
            .await?;
        let kept = tail(&ran.output, ANSWER_LINES, ANSWER_BYTES);
        Ok(Answered {
            truncated: ran.truncated || kept.truncated(),
            lines: kept.total,
            output: kept.text,
            exit: ran.exit,
        })
    }

    /// `program` with `args` run in `cwd` (none: the first root), killed
    /// past `timeout`.
    ///
    /// # Errors
    /// As the sandbox's exec.
    pub async fn run(
        &self,
        program: &str,
        args: Vec<String>,
        cwd: Option<SandboxPath>,
        timeout: Duration,
    ) -> Result<Ran, ModuleError> {
        let spec = ExecSpec {
            program: program.to_string(),
            args,
            cwd,
            env: process_env(self.path)
                .into_iter()
                .chain(self.env.iter().cloned())
                .chain(self.agent.iter().cloned())
                .collect(),
            stdin: None,
            timeout,
            output_limit: OUTPUT_LIMIT,
        };
        // A caller that left (a person stopped the turn) takes its command with it.
        let cancel = CancellationToken::new();
        let _left = cancel.clone().drop_guard();
        let out = self
            .sandbox
            .exec(spec, cancel)
            .await?
            .collect(OUTPUT_LIMIT)
            .await?;
        let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.stderr.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&String::from_utf8_lossy(&out.stderr));
        }
        Ok(Ran {
            output,
            exit: out.exit,
            truncated: out.truncated,
        })
    }
}
