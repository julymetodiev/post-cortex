// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The pcx module host (ARCHITECTURE section 7): loads modules through one
//! path ([`Host::start`]) with their manifests validated and slots checked,
//! calls them under a policy (deadline, cancellation, panic capture,
//! concurrency limit), dispatches events and merges the outcomes, and keeps
//! per-module health (T14); runs hook scripts ([`ScriptModule`]) and
//! modules in their own processes ([`ProcessModule`], over a Unix socket)
//! as modules; lists the running modules and changes their configuration
//! through a [`Settings`] port ([`ModuleSettings`]).
//! Nothing in it is specific to memory.

#![forbid(unsafe_code)]

mod breaker;
mod bus;
mod caller;
mod change;
mod delegation;
mod dispatch;
mod error;
mod grant;
mod info;
mod lifecycle;
mod models;
mod pin;
mod policy;
mod process;
mod prompts;
mod registry;
mod reload;
mod remote;
mod route;
mod rpc_tools;
mod script;
mod settings;
mod stats;
mod turns;
mod views;

pub use bus::{EventStore, StoredEvents};
pub use dispatch::{Dispatched, Failure};
pub use error::HostError;
pub use grant::{Grant, ModuleGrants, RootGrant, sandbox_policy};
pub use info::ModuleInfo;
pub use pin::at_load;
pub use process::{
    Direct, Ended, Launch, ModuleLauncher, ModuleProcess, ProcessModule, ProcessSpec, output_lines,
};
pub use remote::RemoteModule;
pub use route::Origin;
pub use script::{ScriptEvent, ScriptModule, ScriptPolicy, ScriptSpec};
pub use settings::{ModuleSettings, Settings, SettingsError};
pub use stats::ModuleHealth;
pub use views::ModuleView;

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use pcx_module_api::model::{ModelInfo, ModelProvider};
use pcx_module_api::sandbox::Policy;
use pcx_module_api::{
    Event, EventKind, LogLine, MemoryWrite, Module, ModuleError, ModuleErrorKind, Prompt,
    PromptContent, Scope, ToolOutput, ToolSpec,
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::delegation::Delegations;
use crate::info::Configured;
use crate::lifecycle::stop_all;
use crate::models::Models;
use crate::policy::Guarded;

/// The running modules, in load order. A call or event works on the set
/// loaded when it began; [`Host::reload`] swaps in a new set.
pub struct Host {
    /// Shared with the modules' handles ([`caller::Caller`]), which hold it
    /// weakly.
    loaded: Arc<ArcSwap<Loaded>>,
    /// The turns modules asked of the agent that run now, across reloads;
    /// shared with the modules' handles.
    delegations: Arc<Delegations>,
    /// One reload at a time.
    reloading: Mutex<()>,
    /// The modules loaded while it runs (`Host::change`): every reload keeps
    /// them, as the configuration it is given does not name them.
    changed: parking_lot::Mutex<Vec<Arc<dyn Module>>>,
    /// The organisation's bus, across reloads; shared with the modules'
    /// handles.
    bus: Arc<bus::HostBus>,
    /// The organisation the host serves (its slug), which every call it
    /// routes names; empty: it serves none (a daemon, a runner).
    tenant: Arc<str>,
    /// The modules its last reload went on without, each with why: a
    /// hosted tenant's that refused their settings or did not start.
    left_out: parking_lot::Mutex<Vec<(String, String)>>,
}

/// One set of running modules and their tools.
#[derive(Default)]
struct Loaded {
    modules: Vec<Arc<Guarded>>,
    /// What each module was started with, in the order of `modules`.
    configured: Vec<Configured>,
    /// The tools MCP clients see, with the index of the module that
    /// offers each: a module's MCP tools when it has them (memory's), else
    /// the RPCs it names as tools.
    tools: Vec<(usize, ToolSpec)>,
    /// The RPCs the modules name as tools (W23): what agents' models and
    /// modules call, and MCP clients but of a module with MCP tools.
    rpcs: Vec<rpc_tools::HostRpc>,
    /// The modules added in place ([`Host::change`]), by id.
    added: Vec<String>,
}

impl Host {
    /// Validate `modules` and start each with its section of `config` and
    /// its policy of `policies`; see [`Host::reload`].
    ///
    /// # Errors
    /// As [`Host::reload`].
    pub async fn start(
        modules: Vec<Arc<dyn Module>>,
        config: &HashMap<String, Value>,
        policies: &HashMap<String, Policy>,
    ) -> Result<Self, HostError> {
        Self::start_with(modules, config, policies, None, "").await
    }

    /// [`Host::start`], the organisation's events kept in `events` (none:
    /// the host keeps no events, and a module's publish is refused), for
    /// the organisation `tenant` (its slug; empty: for none), which every
    /// call the host routes then names.
    ///
    /// # Errors
    /// As [`Host::start`].
    pub async fn start_with(
        modules: Vec<Arc<dyn Module>>,
        config: &HashMap<String, Value>,
        policies: &HashMap<String, Policy>,
        events: Option<Arc<dyn EventStore>>,
        tenant: &str,
    ) -> Result<Self, HostError> {
        let host = Self {
            loaded: Arc::new(ArcSwap::from_pointee(Loaded::default())),
            delegations: Arc::new(Delegations::default()),
            reloading: Mutex::new(()),
            changed: parking_lot::Mutex::new(Vec::new()),
            bus: Arc::new(bus::HostBus::new(events)),
            tenant: Arc::from(tenant),
            left_out: parking_lot::Mutex::new(Vec::new()),
        };
        host.reload(modules, config, policies).await?;
        Ok(host)
    }

    /// Every module's tools an MCP client may call, in load order (names
    /// are unique).
    #[must_use]
    pub fn tools(&self) -> Vec<ToolSpec> {
        self.loaded
            .load()
            .tools
            .iter()
            .filter(|(_, tool)| tool.visibility.model())
            .map(|(_, tool)| tool.clone())
            .collect()
    }

    /// The gRPC path of `tool` when it is an RPC (`Manifest.rpcs`); none
    /// for an MCP tool or an unknown one.
    #[must_use]
    pub fn rpc_path(&self, tool: &str) -> Option<String> {
        rpc_of(&self.loaded.load(), tool).map(|rpc| rpc.tool.path().to_string())
    }

    /// Whether a module offers `tool`.
    #[must_use]
    pub fn has_tool(&self, tool: &str) -> bool {
        self.loaded.load().tools.iter().any(|(_, t)| t.name == tool)
    }

    /// Call `tool` (of the module that offers it) within `scope`'s
    /// deadline for an MCP client (an agent, an API key): not a module's
    /// call, whatever `scope` says (a module calls through its channel). An
    /// RPC goes through the host's channel of an agent, checked by its
    /// route; a tool only a person calls (visibility `app` only) is refused.
    ///
    /// # Errors
    /// `NotFound` for an unknown tool; `Denied` for a tool only a person
    /// calls; the module's error; `DeadlineExceeded` when it ran out of
    /// time or was cancelled; `Internal` when it panicked.
    pub async fn call_for_agent(
        &self,
        scope: &Scope,
        tool: &str,
        input: Value,
    ) -> Result<ToolOutput, ModuleError> {
        let loaded = self.loaded.load_full();
        let (guarded, spec) = module_of(&loaded, tool)?;
        if !spec.visibility.model() {
            return Err(only_for_a_person(tool));
        }
        // An agent's call never acts with a person's role (docs/host.md);
        // the host of an organisation names it.
        let tenant = if self.tenant.is_empty() {
            scope.tenant.clone()
        } else {
            self.tenant.to_string()
        };
        let scope = Scope {
            tenant,
            caller_module: None,
            call_depth: 0,
            role: None,
            ..scope.clone()
        };
        if let Some(rpc) = rpc_of(&loaded, tool) {
            return rpc
                .tool
                .call(self.channel(Origin::Agent), &scope, input)
                .await;
        }
        run_call(guarded, &scope, tool, input).await
    }

    /// The prompts of the running modules that offer them, in load order
    /// (names are unique: the first module that gives one is kept).
    pub async fn prompts(&self, scope: &Scope) -> Vec<Prompt> {
        prompts::list(&self.loaded.load_full().modules, scope).await
    }

    /// Prompt `name`, filled with `arguments`, from the module that lists it.
    ///
    /// # Errors
    /// `NotFound` when no module offers it; the module's error.
    pub async fn prompt(
        &self,
        scope: &Scope,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        prompts::get(&self.loaded.load_full().modules, scope, name, arguments).await
    }

    /// Dispatch `event` (of `kind`) to every module subscribed to it, under
    /// each subscription's deadline and policy, and merge their outcomes.
    pub async fn dispatch(&self, scope: &Scope, kind: EventKind, event: &Event) -> Dispatched {
        let loaded = self.loaded.load_full();
        dispatch::dispatch(&loaded.modules, scope, kind, event, |_| true).await
    }

    /// Dispatch a memory write to the modules whose `MEMORY_WRITE`
    /// subscription follows it (`Subscription.memory`).
    pub async fn dispatch_memory_write(&self, scope: &Scope, write: &MemoryWrite) -> Dispatched {
        let loaded = self.loaded.load_full();
        let event = write.to_event();
        dispatch::dispatch(
            &loaded.modules,
            scope,
            EventKind::MemoryWrite,
            &event,
            |s| write.followed_by(s.memory.as_ref()),
        )
        .await
    }

    /// Health of every module: calls, failures, timeouts, panics, latency.
    #[must_use]
    pub fn health(&self) -> Vec<ModuleHealth> {
        self.loaded
            .load()
            .modules
            .iter()
            .map(|g| g.health())
            .collect()
    }

    /// Every model of the running providers, each named by its provider,
    /// in load order (`Host.ListModels`).
    #[must_use]
    pub fn models(&self) -> Vec<ModelInfo> {
        Models::of(&self.loaded.load().modules).models()
    }

    /// Every running module, in load order: its manifest, its
    /// configuration without secrets and its health.
    #[must_use]
    pub fn modules(&self) -> Vec<ModuleInfo> {
        let loaded = self.loaded.load();
        loaded
            .modules
            .iter()
            .zip(&loaded.configured)
            .map(|(g, c)| c.info(g.module().manifest(), g.health()))
            .collect()
    }

    /// The running module `id` whole, as [`Host::modules`] lists it; none
    /// when no such module runs.
    #[must_use]
    pub fn module(&self, id: &str) -> Option<ModuleInfo> {
        let loaded = self.loaded.load();
        let index = loaded.modules.iter().position(|g| g.id() == id)?;
        let guarded = &loaded.modules[index];
        Some(loaded.configured[index].info(guarded.module().manifest(), guarded.health()))
    }

    /// The log of the running module `id` (W25), the oldest line first;
    /// none when no such module runs.
    #[must_use]
    pub fn module_log(&self, id: &str) -> Option<Vec<LogLine>> {
        let loaded = self.loaded.load();
        loaded
            .modules
            .iter()
            .find(|g| g.id() == id)
            .map(|g| g.log())
    }

    /// What the running module `id` was started with; none when no such
    /// module runs.
    fn configured(&self, id: &str) -> Option<Configured> {
        let loaded = self.loaded.load();
        let index = loaded.modules.iter().position(|g| g.id() == id)?;
        Some(loaded.configured[index].clone())
    }

    /// Stop every module, last loaded first; a module that does not stop
    /// cleanly is logged.
    pub async fn stop(&self) {
        stop_all(&self.loaded.load().modules).await;
    }
}

/// The module of `loaded` that offers `tool`, and the tool.
fn module_of<'a>(
    loaded: &'a Loaded,
    tool: &str,
) -> Result<(&'a Arc<Guarded>, &'a ToolSpec), ModuleError> {
    loaded
        .tools
        .iter()
        .find(|(_, t)| t.name == tool)
        .map(|(module, spec)| (&loaded.modules[*module], spec))
        .ok_or_else(|| ModuleError::new(ModuleErrorKind::NotFound, format!("no tool {tool}")))
}

/// The RPC tool `tool`, if it is one.
fn rpc_of<'a>(loaded: &'a Loaded, tool: &str) -> Option<&'a rpc_tools::HostRpc> {
    loaded.rpcs.iter().find(|rpc| rpc.tool.spec.name == tool)
}

/// The refusal of a tool only a person calls, to an agent or a module.
fn only_for_a_person(tool: &str) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::Denied,
        format!("{tool} is only for a person to call (its visibility is app)"),
    )
}

/// Call MCP tool `tool` of `guarded` under its policy, within `scope`'s
/// deadline.
async fn run_call(
    guarded: &Guarded,
    scope: &Scope,
    tool: &str,
    input: Value,
) -> Result<ToolOutput, ModuleError> {
    guarded
        .run(&scope.cancel, scope.deadline, "call", |m| {
            let scope = scope.clone();
            let tool = tool.to_string();
            Box::pin(async move { m.call_mcp(&scope, &tool, input).await })
        })
        .await
}

#[cfg(test)]
mod tests;
