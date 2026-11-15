// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The trait every module implements.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use pbjson_types::Any;
use serde_json::Value;

use crate::agent::Turns;
use crate::context::ContextEngine;
use crate::grpc::GrpcChannel;
use crate::model::ModelProvider;
use crate::rpc_tool::RpcTool;
use crate::sandbox::{Sandbox, SandboxProvider};
use crate::{
    ApiVersion, Event, EventKind, LogLine, Manifest, ModuleError, ModuleErrorKind, Outcome, Prompt,
    PromptContent, Resource, Scope, ToolOutput, ToolSpec,
};

/// What the host gives a module when it starts.
#[derive(Clone)]
pub struct HostContext {
    /// The module API version the host speaks.
    pub host_api: ApiVersion,
    /// The module's configuration, a message of its `config_type` (empty:
    /// it takes none), which the host has checked.
    pub config: Any,
    /// The module's handle on the host, usable once its start returned.
    pub host: Arc<dyn ModuleHost>,
    /// Its gRPC channel to the host (W23): calls to the services of the
    /// modules it uses, through generated clients, the scope in their
    /// metadata; the host names this module as the caller.
    pub channel: GrpcChannel,
}

impl HostContext {
    /// Its configuration as `M`, the message of its `config_type`; an empty
    /// configuration is `M`'s default.
    ///
    /// # Errors
    /// `InvalidArgument` when it is not an `M`.
    pub fn config<M: prost::Message + Default>(&self) -> Result<M, ModuleError> {
        M::decode(self.config.value.as_ref()).map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("the configuration is not a {}: {e}", self.config.type_url),
            )
        })
    }
}

impl fmt::Debug for HostContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostContext")
            .field("host_api", &self.host_api)
            .field("config", &self.config.type_url)
            .finish_non_exhaustive()
    }
}

/// A tool of a module this one uses: one of its RPCs, which the module
/// calls through its channel ([`RpcTool::call`]).
#[derive(Debug, Clone)]
pub struct UsedTool {
    /// The module that offers it.
    pub module: String,
    /// The tool.
    pub tool: RpcTool,
}

/// A module's handle as the source of the services a host serves it on its
/// callback socket (its models, its context engine, the agent's turns),
/// each through the handle's own checks.
#[derive(Clone)]
pub struct ThroughHost {
    host: Arc<dyn ModuleHost>,
}

impl ThroughHost {
    /// The services of `host`, a module's handle.
    #[must_use]
    pub fn new(host: Arc<dyn ModuleHost>) -> Self {
        Self { host }
    }

    /// The handle.
    #[must_use]
    pub fn host(&self) -> &Arc<dyn ModuleHost> {
        &self.host
    }
}

/// What a running module may ask of its host (ARCHITECTURE section 7,
/// modules together).
#[async_trait]
pub trait ModuleHost: Send + Sync {
    /// The manifests of the running modules, in load order (checked by the
    /// host; [`crate::DeclaredKind::of`] reads their document kinds).
    ///
    /// # Errors
    /// `FailedPrecondition` before this module runs; `Unavailable` when the
    /// host cannot be reached.
    async fn manifests(&self) -> Result<Vec<Manifest>, ModuleError>;

    /// The sandbox of this module's policy: every file and process access
    /// it makes goes through it.
    ///
    /// # Errors
    /// `FailedPrecondition` without a module in the `sandbox` slot or
    /// before this module runs; `Unavailable` when the host cannot be
    /// reached.
    async fn sandbox(&self) -> Result<Arc<dyn Sandbox>, ModuleError>;

    /// The models of the running providers, each request routed to the
    /// provider that lists its model.
    ///
    /// # Errors
    /// `Denied` for a module outside the `agent` and `context_engine`
    /// slots; `FailedPrecondition` before this module runs; `Unavailable`
    /// when the host cannot be reached.
    async fn model(&self) -> Result<Arc<dyn ModelProvider>, ModuleError>;

    /// The context engine (the module in the `context_engine` slot);
    /// none when no module fills it.
    ///
    /// # Errors
    /// `Denied` for a module outside the `agent` slot;
    /// `FailedPrecondition` before this module runs; `Unavailable` when the
    /// host cannot be reached.
    async fn context(&self) -> Result<Option<Arc<dyn ContextEngine>>, ModuleError>;

    /// The turns of the agent (the module in the `agent` slot), for a
    /// module whose `uses` names it; each runs under this module's reach
    /// and [`crate::agent::TurnGrant`], which the host checks.
    ///
    /// # Errors
    /// `Denied` for a module that does not use the agent;
    /// `FailedPrecondition` before this module runs, or without an agent;
    /// `Unavailable` when the host cannot be reached. For a module process
    /// the handle reaches the host on its callback socket, which checks on
    /// the first turn.
    async fn turns(&self) -> Result<Arc<dyn Turns>, ModuleError>;

    /// The turns module `module` runs in its own name (`Manifest.turns`:
    /// the team's, a member's turn), for a module whose `uses` names it.
    /// Each runs as that module runs it, under that module's reach and
    /// grant, not this one's: this module only asks.
    ///
    /// # Errors
    /// `Denied` for a module that does not use `module`; `NotFound` when no
    /// running module `module` runs turns; `FailedPrecondition` before this
    /// module runs; `Unavailable` when the host cannot be reached. For a
    /// module process the handle reaches the host on its callback socket,
    /// which checks on the first turn.
    async fn turns_of(&self, module: &str) -> Result<Arc<dyn Turns>, ModuleError>;

    /// The tools of the modules this one uses that an agent may call, each
    /// with its module: the RPCs it may call through its channel in `scope`
    /// (in a module's turn, `Scope.turn`, those that module's reach and
    /// grant allow), which the host's route checks again on each call.
    ///
    /// # Errors
    /// `FailedPrecondition` before this module runs; `Denied` for a turn
    /// the host does not run; `Unavailable` when the host cannot be
    /// reached.
    async fn tools(&self, scope: &Scope) -> Result<Vec<UsedTool>, ModuleError>;

    /// The organisation's bus as this module sees it: the events its
    /// manifest declares, emitted; those it follows, fed.
    ///
    /// # Errors
    /// `FailedPrecondition` before this module runs, or on a host that
    /// keeps no events; `Unavailable` when the host cannot be reached.
    async fn bus(&self) -> Result<Arc<dyn crate::bus::Bus>, ModuleError>;

    /// Send `event` (of `kind`) to the modules subscribed to it, under
    /// their policies, and merge their outcomes (a failure is logged by the
    /// host, and blocks when its module is fail-closed).
    ///
    /// # Errors
    /// `Denied` for a module outside the `agent` slot;
    /// `FailedPrecondition` before this module runs; `Unavailable` when the
    /// host cannot be reached.
    async fn dispatch(
        &self,
        scope: &Scope,
        kind: EventKind,
        event: &Event,
    ) -> Result<Outcome, ModuleError>;

    /// Say what stops this module from doing its work, as it found after
    /// it started, for a person and with what would fix it; empty: nothing
    /// does. Its health carries it until the next report (docs/host.md, a
    /// module's problem).
    ///
    /// # Errors
    /// `FailedPrecondition` before this module runs; `Unavailable` when the
    /// host cannot be reached.
    async fn report(&self, problem: &str) -> Result<(), ModuleError>;
}

/// A pcx module: memory, a model provider, tools, a channel, ... The host
/// calls it under the deadline and failure policy its manifest declares;
/// a panic is caught at the host boundary.
#[async_trait]
pub trait Module: Send + Sync {
    /// What the module is; read once, before [`Module::start`].
    fn manifest(&self) -> &Manifest;

    /// Start the module; nothing else is called until it returns.
    ///
    /// # Errors
    /// The module cannot run (its configuration or a backend).
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError>;

    /// Its MCP tools (JSON Schema in, text and JSON out), which only MCP
    /// clients call: memory's, its published surface (owner 2026-10-26).
    /// Every other module's API is its RPCs (`Manifest.rpcs`); none by
    /// default.
    fn mcp_tools(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    /// Call one of its MCP tools.
    ///
    /// # Errors
    /// See [`ModuleError`]; `NotFound` by default.
    async fn call_mcp(
        &self,
        _scope: &Scope,
        tool: &str,
        _input: Value,
    ) -> Result<ToolOutput, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("{} offers no MCP tool {tool}", self.manifest().id),
        ))
    }

    /// Answer an event the module subscribed to.
    ///
    /// # Errors
    /// See [`ModuleError`]; the host applies the subscription's policy.
    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError>;

    /// The HTML resource at `uri`, one of its views' `ui://` URIs
    /// ([`crate::view::HTML_MIME`]).
    ///
    /// # Errors
    /// `NotFound` for a URI it does not serve.
    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError>;

    /// The prompts it offers now, in its order; they may change while it
    /// runs. None for a module without prompts.
    ///
    /// # Errors
    /// See [`ModuleError`].
    async fn prompts(&self, _scope: &Scope) -> Result<Vec<Prompt>, ModuleError> {
        Ok(Vec::new())
    }

    /// Its prompt `name`, filled with `arguments`.
    ///
    /// # Errors
    /// `NotFound` for a name it does not offer; `InvalidArgument` for
    /// arguments the prompt does not take.
    async fn prompt(
        &self,
        _scope: &Scope,
        name: &str,
        _arguments: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("{} offers no prompt {name}", self.manifest().id),
        ))
    }

    /// Stop the module; it may finish what is in flight.
    ///
    /// # Errors
    /// It did not stop cleanly.
    async fn stop(&self) -> Result<(), ModuleError>;

    /// The sandboxes it provides, for a module of the `sandbox` slot; none
    /// for any other.
    fn sandbox_provider(&self) -> Option<Arc<dyn SandboxProvider>> {
        None
    }

    /// The models it serves, for a module of kind `model_provider`; none
    /// for any other.
    fn model_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        None
    }

    /// The turns it runs: an agent's, or those of a module that runs turns
    /// in its own name (`Manifest.turns`, the chief); none for any other.
    fn turns(&self) -> Option<Arc<dyn Turns>> {
        None
    }

    /// Its context engine, for a module of kind `context_engine`; none for
    /// any other.
    fn context_engine(&self) -> Option<Arc<dyn ContextEngine>> {
        None
    }

    /// Its gRPC services (`Manifest.services`, W23), as one channel the
    /// host routes their calls to; none for a module without.
    fn services(&self) -> Option<GrpcChannel> {
        None
    }

    /// What its own process wrote and the host saw of that process (W25),
    /// the oldest first; none for a module in the host's process.
    fn output(&self) -> Vec<LogLine> {
        Vec::new()
    }
}
