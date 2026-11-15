// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `agent` module: its manifest (kind `agent`, its settings, the
//! modules whose tools it may call) and its turns, run over the agent
//! service with its host handle's ports and models.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, Turns};
use pcx_module_api::{
    API_VERSION, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind, ModuleHost,
    ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::agent::AgentConfig;
use tokio::sync::mpsc;

use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::model::ModelInfo;

use crate::adapters::HostPorts;
use crate::service::{Agent, Settings};

/// The module's id.
const MODULE_ID: &str = crate::domain::AGENT;

/// The model a configuration names none of.
const DEFAULT_MODEL: &str = "glm-5.3";

/// The system prompt a configuration names none of.
const DEFAULT_SYSTEM: &str = "You are pcx's agent. Work with the tools you have; read before you change, and say what you did.";

/// The most tokens of one answer, unless configured.
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// The most model calls of one turn, unless configured.
const DEFAULT_MAX_STEPS: u32 = 50;

/// The modules whose tools the agent may call (those that run).
const USES: [&str; 14] = [
    "post-cortex.memory",
    "tools-fs",
    "tools-shell",
    "tools-git",
    "board",
    "skills",
    "team",
    "observe",
    "ops",
    "scheduler",
    "workflow",
    "mail",
    "http",
    "dynamic",
];

/// The full name of its configuration message.
const CONFIG: &str = "pcx.agent.v1.AgentConfig";

/// `text`, or `default` when it is empty (unset).
fn or_default(text: String, default: &str) -> String {
    if text.trim().is_empty() {
        default.to_string()
    } else {
        text
    }
}

/// `count`, or `default` when it is 0 (unset).
fn or_count(count: u32, default: u32) -> u32 {
    if count == 0 { default } else { count }
}

/// What a started agent works with.
#[derive(Clone)]
struct Running {
    host: Arc<dyn ModuleHost>,
    /// Its host channel, to memory's messages.
    channel: GrpcChannel,
    settings: Settings,
}

/// The agent's turns, once it runs.
#[derive(Default)]
struct AgentTurns {
    running: Mutex<Option<Running>>,
}

#[async_trait]
impl Turns for AgentTurns {
    async fn models(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        let running = self.running.lock().clone().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "the agent has not started",
            )
        })?;
        Ok(running.host.model().await?.models())
    }

    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        let running = self.running.lock().clone().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "the agent has not started",
            )
        })?;
        // A turn's calls and events are its session's (memory captures
        // the turn in it).
        let scope = &Scope {
            session_id: Some(request.session.clone()),
            ..scope.clone()
        };
        let models = running.host.model().await?;
        let context = running.host.context().await?;
        let ports = HostPorts::new(
            running.host.as_ref(),
            running.channel.clone(),
            scope,
            request.workspace.clone(),
        );
        Agent {
            models: models.as_ref(),
            tools: &ports,
            hooks: &ports,
            transcript: &ports,
            context: context.as_deref(),
            settings: &running.settings,
            scope,
        }
        .run(request, events)
        .await
    }
}

/// The `agent` module.
pub struct AgentModule {
    manifest: Manifest,
    turns: Arc<AgentTurns>,
}

impl AgentModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Runs turns: the model and the tools a turn may use.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Agent.into(),
                descriptors: post_cortex_proto::AGENT_DESCRIPTOR_SET.to_vec(),
                config_type: CONFIG.to_string(),
                uses: USES.iter().map(|m| (*m).to_string()).collect(),
                ..Manifest::default()
            },
            turns: Arc::new(AgentTurns::default()),
        }
    }
}

impl Default for AgentModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for AgentModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: AgentConfig = ctx.config()?;
        let settings = Settings {
            model: or_default(config.model, DEFAULT_MODEL),
            system: or_default(config.system, DEFAULT_SYSTEM),
            max_tokens: or_count(config.max_tokens, DEFAULT_MAX_TOKENS),
            max_steps: or_count(config.max_steps, DEFAULT_MAX_STEPS),
        };
        *self.turns.running.lock() = Some(Running {
            host: ctx.host,
            channel: ctx.channel,
            settings,
        });
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("the agent serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        *self.turns.running.lock() = None;
        Ok(())
    }

    fn turns(&self) -> Option<Arc<dyn Turns>> {
        Some(Arc::clone(&self.turns) as Arc<dyn Turns>)
    }
}
