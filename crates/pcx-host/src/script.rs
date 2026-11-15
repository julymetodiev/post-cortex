// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The hook-script runner (ARCHITECTURE section 7): a short-lived script
//! answers events as a Claude Code hook does. It gets the event as JSON on
//! stdin (the scope's session and workspace in `PCX_SESSION_ID` and
//! `PCX_WORKSPACE_ID`) and answers on stdout; exit 0 is its answer (a JSON
//! outcome, or text that SessionStart and UserPromptSubmit add as context),
//! exit 2 blocks with stderr as the reason, any other exit is a failure.
//! A script is pinned by the SHA-256 of its file: one that does not match
//! is refused at load, and one that changed since is refused at each run,
//! until its new hash is approved in the configuration.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use pcx_module_api::{
    API_VERSION, Event, FailurePolicy, HookSpecificOutput, HostContext, Manifest, Module,
    ModuleError, ModuleErrorKind, ModuleKind, Outcome, Resource, Scope, Subscription, event_kind,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::grant::{self, Grant};
use crate::{HostError, pin};

/// The exit status with which a script blocks (Claude Code's).
const BLOCK_EXIT: i32 = 2;

/// A hook script as the configuration declares it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptSpec {
    /// The module id.
    pub id: String,
    /// The executable.
    pub command: PathBuf,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// The approved SHA-256 of the executable's file, in hex.
    pub sha256: String,
    /// The events it answers.
    pub events: Vec<ScriptEvent>,
    /// What it may do; without `read_conversation` its events come without
    /// the prompt, the tool's input and result, the answer and the
    /// transcript.
    #[serde(default)]
    pub capabilities: Vec<Grant>,
}

/// One event a script answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptEvent {
    /// The hook name (`PreToolUse`, `SessionStart`, ...).
    pub event: String,
    /// What a failure or a missed deadline does; only a fail-closed script
    /// may block.
    #[serde(default)]
    pub policy: ScriptPolicy,
    /// Longest it may take, in milliseconds; 0: the host's default.
    #[serde(default)]
    pub deadline_ms: u32,
}

/// The failure policy of a script's event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptPolicy {
    /// The turn goes on without the script's answer.
    #[default]
    FailOpen,
    /// The turn is blocked.
    FailClosed,
}

/// A hook script loaded as a module.
pub struct ScriptModule {
    spec: ScriptSpec,
    manifest: Manifest,
}

impl ScriptModule {
    /// The script of `spec`, its file checked against the approved hash.
    ///
    /// # Errors
    /// An event no hook names, a file that cannot be read, or a hash that
    /// does not match.
    pub fn load(spec: ScriptSpec) -> Result<Self, HostError> {
        let mut events = Vec::with_capacity(spec.events.len());
        for e in &spec.events {
            let kind = event_kind(&e.event).ok_or_else(|| HostError::InvalidManifest {
                module: spec.id.clone(),
                reason: format!("no hook named {}", e.event),
            })?;
            let policy = match e.policy {
                ScriptPolicy::FailOpen => FailurePolicy::FailOpen,
                ScriptPolicy::FailClosed => FailurePolicy::FailClosed,
            };
            events.push(Subscription {
                kind: kind.into(),
                deadline_ms: e.deadline_ms,
                policy: policy.into(),
                memory: None,
            });
        }
        let found = pin::at_load(&spec.id, &spec.command, &spec.sha256)?;
        let manifest = Manifest {
            emits: Vec::new(),
            follows: Vec::new(),
            workflow_templates: Vec::new(),
            id: spec.id.clone(),
            version: found,
            api: Some(API_VERSION),
            kind: ModuleKind::Hook.into(),
            events,
            capabilities: grant::capabilities(&spec.capabilities),
            filesystem: Vec::new(),
            views: Vec::new(),
            document_kinds: Vec::new(),
            uses: Vec::new(),
            prompts: false,
            turns: false,
            services: Vec::new(),
            descriptors: Vec::new(),
            rpcs: Vec::new(),
            config_type: String::new(),
            description: format!("Runs {} on its hook events.", spec.command.display()),
        };
        Ok(Self { spec, manifest })
    }

    /// Run the script on `event`.
    async fn run(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        pin::at_run(&self.spec.command, &self.spec.sha256).await?;
        let input = serde_json::to_vec(event).map_err(|e| {
            ModuleError::new(ModuleErrorKind::Internal, format!("event as JSON: {e}"))
        })?;
        let mut command = Command::new(&self.spec.command);
        command
            .args(&self.spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A script past its deadline is killed with the dropped call.
            .kill_on_drop(true);
        for (name, value) in [
            ("PCX_SESSION_ID", &scope.session_id),
            ("PCX_WORKSPACE_ID", &scope.workspace_id),
        ] {
            if let Some(value) = value {
                command.env(name, value);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|e| self.failed(format!("start: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&input)
                .await
                .map_err(|e| self.failed(format!("write the event: {e}")))?;
        }
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| self.failed(format!("wait: {e}")))?;
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        match output.status.code() {
            Some(0) => answer(event, &output.stdout).map_err(|reason| self.failed(reason)),
            Some(BLOCK_EXIT) => Ok(Outcome {
                decision: "block".to_string(),
                reason: stderr,
                hook_specific_output: None,
            }),
            Some(code) => Err(self.failed(format!("exited with {code}: {stderr}"))),
            None => Err(self.failed(format!("killed by a signal: {stderr}"))),
        }
    }

    fn failed(&self, reason: String) -> ModuleError {
        ModuleError::new(
            ModuleErrorKind::Internal,
            format!("hook script {}: {reason}", self.spec.id),
        )
    }
}

/// A script's answer: a JSON outcome, nothing, or text that the events
/// whose output Claude Code adds as context add as context.
fn answer(event: &Event, stdout: &[u8]) -> Result<Outcome, String> {
    let text = String::from_utf8_lossy(stdout);
    let text = text.trim();
    if text.is_empty() {
        return Ok(Outcome::default());
    }
    if text.starts_with('{') {
        return serde_json::from_str(text).map_err(|e| format!("answer is not an outcome: {e}"));
    }
    Ok(match event.hook_event_name.as_str() {
        "SessionStart" | "UserPromptSubmit" => Outcome {
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: event.hook_event_name.clone(),
                additional_context: text.to_string(),
            }),
            ..Outcome::default()
        },
        _ => Outcome::default(),
    })
}

#[async_trait]
impl Module for ScriptModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        self.run(scope, event).await
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("hook script {} serves no resource {uri}", self.spec.id),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests;
