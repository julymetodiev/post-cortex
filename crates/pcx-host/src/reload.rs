// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The one load path of a host's modules (docs/host.md): a new set
//! validated, started and swapped in, the modules it no longer has
//! stopped; what a module refuses of its settings (a module's problem),
//! and the modules an organisation's host goes on without.

use std::collections::HashMap;
use std::sync::Arc;

use pcx_module_api::sandbox::Policy;
use pcx_module_api::{API_VERSION, HostContext, Module, ModuleKind};
use serde_json::Value;

use crate::info::Configured;
use crate::lifecycle::{start_each, stop_all};
use crate::policy::Guarded;
use crate::registry::granted;
use crate::route::Origin;
use crate::{Host, HostError, Loaded, caller, info, registry, rpc_tools};

impl Host {
    /// Make `modules` the running set, the one load path: validate them
    /// (API version, kind, events, exclusive slots, unique ids), keep each
    /// that already runs (the same instance), start the others at once
    /// with their section of `config` (keyed by module id; none: `null`)
    /// once every one fits its module's schema, swap the set in, then stop the
    /// modules it no longer has, last loaded first. A module's policy in
    /// `policies` (keyed by module id) is what the operator grants it, its
    /// sandbox's roots, network and processes; a module named there is
    /// granted nothing else, one not named is granted nothing.
    ///
    /// A running module whose new configuration or start was refused keeps
    /// running with the one before, and its health says so until a reload
    /// applies its settings (docs/host.md, a module's problem).
    ///
    /// The host of an organisation (a hosted tenant's, [`Host::start_with`]
    /// with its slug) goes on without a module, other than memory, whose
    /// configuration does not fit or that fails to start: the others run,
    /// the one that ran before stays, and [`Host::left_out`] names those
    /// that do not run. One organisation's wrong setting must not stop its
    /// memory and its other modules (docs/host.md, a module's problem).
    ///
    /// # Errors
    /// A manifest is refused, a configuration does not fit its module's
    /// schema, a module fails to start, or two modules offer a tool of one
    /// name; the modules this call started are stopped and the running set
    /// stays.
    pub async fn reload(
        &self,
        modules: Vec<Arc<dyn Module>>,
        config: &HashMap<String, Value>,
        policies: &HashMap<String, Policy>,
    ) -> Result<(), HostError> {
        let _one_at_a_time = self.reloading.lock().await;
        let applied = self.swap(modules, config, policies).await;
        let running = self.loaded.load();
        let unapplied = |refused: &HostError| {
            format!(
                "its new settings were not applied, and it runs with the ones before: {refused}"
            )
        };
        match &applied {
            Ok(refused) => {
                let of = |id: &str| refused.iter().find(|r| r.refused_module() == Some(id));
                for module in &running.modules {
                    module.unapplied(&of(module.id()).map(unapplied).unwrap_or_default());
                }
                *self.left_out.lock() = refused
                    .iter()
                    .filter_map(|r| Some((r.refused_module()?.to_string(), r.to_string())))
                    .filter(|(id, _)| !running.modules.iter().any(|g| g.id() == id))
                    .collect();
            }
            Err(refused) => {
                let named = refused.refused_module();
                if let Some(module) = running.modules.iter().find(|g| Some(g.id()) == named) {
                    module.unapplied(&unapplied(refused));
                }
            }
        }
        applied.map(|_| ())
    }

    /// The modules the last reload went on without, each with why: those
    /// of an organisation's host that refused their settings or did not
    /// start, and do not run.
    #[must_use]
    pub fn left_out(&self) -> Vec<(String, String)> {
        self.left_out.lock().clone()
    }

    /// [`Host::reload`]'s swap of the running set, under its lock: the
    /// refusals it went on without (an organisation's host only).
    async fn swap(
        &self,
        modules: Vec<Arc<dyn Module>>,
        config: &HashMap<String, Value>,
        policies: &HashMap<String, Policy>,
    ) -> Result<Vec<HostError>, HostError> {
        // An organisation's host goes on without a module of its own
        // choice that is at fault; memory is not one.
        let spared = |guarded: &Guarded| {
            !self.tenant.is_empty()
                && guarded.module().manifest().kind != i32::from(ModuleKind::Memory)
        };
        let mut refused: Vec<HostError> = Vec::new();
        let mut modules = modules;
        let added: Vec<String> = {
            let changed = self.changed.lock();
            modules.extend(changed.iter().cloned());
            changed.iter().map(|m| m.manifest().id.clone()).collect()
        };
        let schemas = registry::validate(&API_VERSION, &modules, policies)?;
        let running = self.loaded.load_full();
        let mut next = Vec::with_capacity(modules.len());
        let mut configured = Vec::with_capacity(modules.len());
        let mut starting = Vec::new();
        for (module, schema) in modules.into_iter().zip(schemas) {
            if let Some(kept) = running
                .modules
                .iter()
                .position(|g| Arc::ptr_eq(g.module(), &module))
            {
                next.push(Arc::clone(&running.modules[kept]));
                configured.push(running.configured[kept].clone());
                continue;
            }
            let guarded = Arc::new(Guarded::new(module));
            let section = config.get(guarded.id()).unwrap_or(&Value::Null);
            let read = match info::read(schema.as_ref(), section) {
                Ok(read) => read,
                Err(error) => {
                    let module = guarded.id().to_string();
                    let refusal = HostError::InvalidConfig { module, error };
                    if !spared(&guarded) {
                        return Err(refusal);
                    }
                    // The one that ran before stays.
                    if let Some(before) =
                        running.modules.iter().position(|g| g.id() == guarded.id())
                    {
                        next.push(Arc::clone(&running.modules[before]));
                        configured.push(running.configured[before].clone());
                    }
                    refused.push(refusal);
                    continue;
                }
            };
            let channel = self.channel(Origin::Module(guarded.id().to_string()));
            let ctx = HostContext {
                host_api: API_VERSION,
                config: read.config.clone(),
                host: Arc::new(caller::Caller::new(
                    guarded.id(),
                    Arc::downgrade(&self.loaded),
                    Arc::clone(&self.delegations),
                    Arc::clone(&self.bus),
                )),
                channel,
            };
            starting.push((Arc::clone(&guarded), ctx));
            configured.push(Configured {
                schema,
                config: read.config,
                shown: read.shown,
                policy: granted(policies, guarded.id()),
                grant: read.grant,
                persons: read.persons,
            });
            next.push(guarded);
        }
        let (started, failed) = start_each(starting).await;
        for (guarded, error) in failed {
            let refusal = HostError::StartFailed {
                module: guarded.id().to_string(),
                error,
            };
            if !spared(&guarded) {
                stop_all(&started).await;
                return Err(refusal);
            }
            // Out of the set, the one that ran before in its place.
            if let Some(at) = next.iter().position(|g| Arc::ptr_eq(g, &guarded)) {
                next.remove(at);
                configured.remove(at);
                if let Some(before) = running.modules.iter().position(|g| g.id() == guarded.id()) {
                    next.insert(at, Arc::clone(&running.modules[before]));
                    configured.insert(at, running.configured[before].clone());
                }
            }
            refused.push(refusal);
        }
        let rpcs = match rpc_tools::of(&next) {
            Ok(rpcs) => rpcs,
            Err(e) => {
                stop_all(&started).await;
                return Err(e);
            }
        };
        let tools = match registry::tools(&next, &rpcs) {
            Ok(tools) => tools,
            Err(e) => {
                stop_all(&started).await;
                return Err(e);
            }
        };
        self.loaded.store(Arc::new(Loaded {
            modules: next,
            configured,
            tools,
            rpcs,
            added,
        }));
        let current = self.loaded.load();
        let gone: Vec<Arc<Guarded>> = running
            .modules
            .iter()
            .filter(|old| !current.modules.iter().any(|g| Arc::ptr_eq(g, old)))
            .cloned()
            .collect();
        stop_all(&gone).await;
        Ok(refused)
    }
}
