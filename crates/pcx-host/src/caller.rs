// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A running module's handle on its host (ARCHITECTURE section 7, modules
//! together): the tools an agent may call of the modules it uses (the
//! agent's, in a module's turn, under that module's reach), which it calls
//! as RPCs through its channel; the manifests of the
//! running set; the sandbox of its policy; the models, for the agent and the
//! context engine; the agent's turns, each under its own token.

use std::sync::{Arc, Weak};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use pcx_module_api::agent::Turns;
use pcx_module_api::bus::Bus;
use pcx_module_api::context::ContextEngine;
use pcx_module_api::model::ModelProvider;
use pcx_module_api::sandbox::Sandbox;
use pcx_module_api::{
    Capability, Event, EventKind, Manifest, ModuleError, ModuleErrorKind, ModuleHost, ModuleKind,
    Outcome, Scope, UsedTool,
};

use crate::Loaded;
use crate::bus::{HostBus, ModuleBus};
use crate::delegation::{Delegated, Delegations, Reach, reach, reaches};
use crate::models::Models;
use crate::policy::Guarded;

/// The handle of the module `module`.
pub struct Caller {
    module: String,
    loaded: Weak<ArcSwap<Loaded>>,
    delegations: Arc<Delegations>,
    bus: Arc<HostBus>,
}

impl Caller {
    /// The handle of `module` on the host whose running set is `loaded`,
    /// whose modules' running turns are `delegations` and whose bus is
    /// `bus`.
    pub fn new(
        module: &str,
        loaded: Weak<ArcSwap<Loaded>>,
        delegations: Arc<Delegations>,
        bus: Arc<HostBus>,
    ) -> Self {
        Self {
            module: module.to_string(),
            loaded,
            delegations,
            bus,
        }
    }

    /// What this module's calls in `scope` reach (see [`reach`]).
    fn reach<'a>(
        &self,
        loaded: &'a Loaded,
        caller: &Guarded,
        scope: &Scope,
    ) -> Result<Reach<'a>, ModuleError> {
        reach(
            loaded,
            &self.delegations,
            caller,
            scope.turn.as_deref(),
            scope.author.as_deref(),
        )
    }

    /// The running set, and this module in it.
    fn running(&self) -> Result<(Arc<Loaded>, Arc<Guarded>), ModuleError> {
        let loaded = self
            .loaded
            .upgrade()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "the host has stopped"))?
            .load_full();
        let caller = loaded
            .modules
            .iter()
            .find(|g| g.id() == self.module)
            .cloned()
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!("module {} is not running", self.module),
                )
            })?;
        Ok((loaded, caller))
    }
}

#[async_trait]
impl ModuleHost for Caller {
    async fn sandbox(&self) -> Result<Arc<dyn Sandbox>, ModuleError> {
        let (loaded, _) = self.running()?;
        let policy = loaded
            .modules
            .iter()
            .position(|g| g.id() == self.module)
            .map(|at| loaded.configured[at].policy.clone())
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!("module {} is not running", self.module),
                )
            })?;
        let provider = loaded
            .modules
            .iter()
            .find_map(|g| g.module().sandbox_provider())
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    "no module fills the sandbox slot: files and processes are refused",
                )
            })?;
        provider.bind(&self.module, policy)
    }

    async fn model(&self) -> Result<Arc<dyn ModelProvider>, ModuleError> {
        let (loaded, caller) = self.running()?;
        let manifest = caller.module().manifest();
        let kind = ModuleKind::try_from(manifest.kind);
        // A module that asks for the models (its grant checked when it
        // loaded) has them as the agent and the context engine do.
        let asks = manifest.capabilities.contains(&Capability::Models.into());
        if !asks && !matches!(kind, Ok(ModuleKind::Agent | ModuleKind::ContextEngine)) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!(
                    "module {} is not in the agent or context_engine slot and does not ask for the models: models are refused",
                    self.module
                ),
            ));
        }
        Ok(Arc::new(Models::of(&loaded.modules)))
    }

    async fn turns(&self) -> Result<Arc<dyn Turns>, ModuleError> {
        let (loaded, caller) = self.running()?;
        let agent = loaded
            .modules
            .iter()
            .find(|g| ModuleKind::try_from(g.module().manifest().kind) == Ok(ModuleKind::Agent))
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    "no module fills the agent slot",
                )
            })?;
        if !pcx_module_api::uses(&caller.module().manifest().uses, agent.id()) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!(
                    "module {} does not use the agent {}",
                    self.module,
                    agent.id()
                ),
            ));
        }
        let turns = agent.module().turns().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("the agent {} runs no turns", agent.id()),
            )
        })?;
        Ok(Arc::new(Delegated::new(
            turns,
            &self.module,
            Arc::clone(&self.delegations),
        )))
    }

    async fn turns_of(&self, module: &str) -> Result<Arc<dyn Turns>, ModuleError> {
        let (loaded, caller) = self.running()?;
        if !pcx_module_api::uses(&caller.module().manifest().uses, module) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!("module {} does not use module {module}", self.module),
            ));
        }
        // That module's own turns, as it runs them: it asks the agent under
        // its own token, so this module's reach adds nothing to them.
        crate::turns::in_own_name(&loaded, module).ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::NotFound,
                format!("no running module {module} runs turns in its own name"),
            )
        })
    }

    async fn context(&self) -> Result<Option<Arc<dyn ContextEngine>>, ModuleError> {
        let (loaded, caller) = self.running()?;
        if ModuleKind::try_from(caller.module().manifest().kind) != Ok(ModuleKind::Agent) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!(
                    "module {} is not in the agent slot: the context engine is refused",
                    self.module
                ),
            ));
        }
        Ok(loaded
            .modules
            .iter()
            .find_map(|g| g.module().context_engine()))
    }

    async fn tools(&self, scope: &Scope) -> Result<Vec<UsedTool>, ModuleError> {
        let (loaded, caller) = self.running()?;
        let uses = &caller.module().manifest().uses;
        let reach = self.reach(&loaded, &caller, scope)?;
        Ok(loaded
            .rpcs
            .iter()
            .filter(|rpc| {
                let callee = loaded.modules[rpc.module].id();
                let spec = &rpc.tool.spec;
                reaches(uses, &loaded.added, callee)
                    && reach.allows(callee, &spec.name, spec.visibility.model())
            })
            .map(|rpc| UsedTool {
                module: loaded.modules[rpc.module].id().to_string(),
                tool: rpc.tool.clone(),
            })
            .collect())
    }

    async fn dispatch(
        &self,
        scope: &Scope,
        kind: EventKind,
        event: &Event,
    ) -> Result<Outcome, ModuleError> {
        let (loaded, caller) = self.running()?;
        if ModuleKind::try_from(caller.module().manifest().kind) != Ok(ModuleKind::Agent) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!(
                    "module {} is not in the agent slot: it sends no events",
                    self.module
                ),
            ));
        }
        let dispatched =
            crate::dispatch::dispatch(&loaded.modules, scope, kind, event, |_| true).await;
        for failure in &dispatched.failures {
            tracing::warn!(
                module = %failure.module,
                error = %failure.error.message,
                blocked = failure.blocked,
                "a module failed the agent's event"
            );
        }
        Ok(dispatched.outcome)
    }

    async fn manifests(&self) -> Result<Vec<Manifest>, ModuleError> {
        let (loaded, _) = self.running()?;
        Ok(loaded
            .modules
            .iter()
            .map(|g| g.module().manifest().clone())
            .collect())
    }

    async fn report(&self, problem: &str) -> Result<(), ModuleError> {
        let (_, caller) = self.running()?;
        caller.report(problem);
        Ok(())
    }

    async fn bus(&self) -> Result<Arc<dyn Bus>, ModuleError> {
        self.running()?;
        Ok(Arc::new(ModuleBus::new(
            &self.module,
            Arc::clone(&self.bus),
            Weak::clone(&self.loaded),
        )))
    }
}
