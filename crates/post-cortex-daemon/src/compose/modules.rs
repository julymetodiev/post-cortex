// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The modules the daemon's host runs (ARCHITECTURE section 7): memory, the
//! sandbox, the hook scripts (`[[hooks]]`), the module processes
//! (`[[modules]]`) and the built-in modules (`[[builtins]]`), each with
//! the configuration and the policy its entry grants.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use pcx_dynamic::{DynamicModule, LauncherConfig};
use pcx_host::{
    Grant, Host, HostError, ProcessSpec, RootGrant, ScriptModule, ScriptSpec, sandbox_policy,
};
use pcx_module_api::Module;
use pcx_module_api::grpc;
use pcx_module_api::sandbox::Policy;
use pcx_sandbox::{Launcher, SandboxModule};
use post_cortex_memory::{Memory, MemoryModule};
use post_cortex_ports::DocumentKinds;
use post_cortex_transport::memory::PcxGrpcService;
use post_cortex_transport::reach::Reached;

use crate::compose::BUILD;
use crate::compose::builtin::Builtin;
use crate::compose::launch::CatalogLauncher;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A built-in module as `[[builtins]]` grants it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinSpec {
    /// Which module.
    pub id: Builtin,
    /// What it may do (`filesystem`, `spawn`, `network`).
    #[serde(default)]
    pub grants: Vec<Grant>,
    /// The directories it may use (with the `filesystem` grant).
    #[serde(default)]
    pub filesystem: Vec<RootGrant>,
    /// Its configuration (a table).
    #[serde(default)]
    pub config: Option<Value>,
}

/// The modules the daemon's host runs, each configured one with the entry
/// it was loaded from.
pub struct Modules {
    memory: Arc<dyn Module>,
    /// The running modules' document kinds, as memory reads them.
    kinds: Arc<dyn DocumentKinds>,
    /// The memory the memory module is over: its documents are migrated
    /// once the modules run.
    store: Arc<Memory>,
    sandbox: Option<Arc<dyn Module>>,
    /// The names under every module's roots none reaches (`[runner] deny`).
    deny: Vec<String>,
    hooks: Vec<(ScriptSpec, Arc<dyn Module>)>,
    processes: Vec<(ProcessSpec, Arc<dyn Module>)>,
    builtins: Vec<(BuiltinSpec, Arc<dyn Module>)>,
    /// The `dynamic` module, with its configuration, when this host runs one.
    dynamic: Option<(Arc<dyn Module>, Value)>,
    /// Modules served elsewhere (a runner's, docs/connect.md section 7),
    /// granted nothing here.
    remote: Vec<Arc<dyn Module>>,
    /// The organisation the host serves (its slug); empty: none.
    tenant: String,
}

impl Modules {
    /// The memory module over `memory`, alone, serving `pcx.v1` to the
    /// other modules (W23).
    ///
    /// # Errors
    /// A memory tool's schema could not be built.
    pub fn memory(memory: Arc<Memory>) -> Result<Self, String> {
        let module = MemoryModule::new(Arc::clone(&memory)).map_err(|e| e.to_string())?;
        let kinds = module.kinds();
        let service = PcxGrpcService::new(Arc::clone(&memory), BUILD, Arc::clone(&kinds));
        // The modules' calls carry the reach of the call they serve.
        let module = module.serving(grpc::routes(
            tonic::service::Routes::new(Reached::new(
                service.clone().into_server(),
                Arc::clone(&memory),
            ))
            .add_service(Reached::new(
                service.into_blobs_server(),
                Arc::clone(&memory),
            )),
        ));
        Ok(Self {
            memory: Arc::new(module),
            kinds,
            store: memory,
            sandbox: None,
            deny: Vec::new(),
            hooks: Vec::new(),
            processes: Vec::new(),
            builtins: Vec::new(),
            remote: Vec::new(),
            dynamic: None,
            tenant: String::new(),
        })
    }

    /// These modules with the sandbox of this platform in its slot, the
    /// modules' scratch directories under `scratch_root`, the names in
    /// `deny` out of every module's reach, processes run as `user` (a
    /// FreeBSD jail's, `[sandbox] user`).
    ///
    /// # Errors
    /// The `pcx` binary (the Linux and FreeBSD sandbox's helper) cannot be
    /// found, or `user` is named where no sandbox switches users.
    pub fn sandboxed(
        self,
        scratch_root: &Path,
        deny: Vec<String>,
        user: Option<String>,
    ) -> Result<Self, String> {
        let launcher =
            Launcher::platform(user).map_err(|e| format!("the sandbox does not start: {e}"))?;
        Ok(Self {
            sandbox: Some(Arc::new(SandboxModule::new(launcher, scratch_root))),
            deny,
            ..self
        })
    }

    /// These modules with `remote`, modules served elsewhere (a runner's
    /// tool modules in a hosted tenant), granted nothing here.
    #[must_use]
    pub fn with_remote(self, remote: Vec<Arc<dyn Module>>) -> Self {
        Self { remote, ..self }
    }

    /// These modules on the host of the organisation `tenant` (its slug, a
    /// hosted tenant's), which every call the host routes names.
    #[must_use]
    pub fn for_tenant(self, tenant: &str) -> Self {
        Self {
            tenant: tenant.to_string(),
            ..self
        }
    }

    /// These modules with `dynamic` (docs/modules/dynamic.md), with the
    /// configuration `config`, the one module whose constructor needs the
    /// launcher it reaches.
    #[must_use]
    pub fn with_dynamic(self, launcher: LauncherConfig, config: Value) -> Self {
        Self {
            dynamic: Some((Arc::new(DynamicModule::new(launcher)), config)),
            ..self
        }
    }

    /// Whether the configured modules are `hooks`, `processes` and
    /// `builtins`.
    #[must_use]
    pub fn are(
        &self,
        hooks: &[ScriptSpec],
        processes: &[ProcessSpec],
        builtins: &[BuiltinSpec],
    ) -> bool {
        self.hooks.iter().map(|(s, _)| s).eq(hooks)
            && self.processes.iter().map(|(s, _)| s).eq(processes)
            && self.builtins.iter().map(|(s, _)| s).eq(builtins)
    }

    /// These modules with every module process of `processes` launched at
    /// once (a hosted tenant's: no hook scripts, no built-in modules),
    /// through `catalog` when the cloud has its launcher: a process whose
    /// spec has not changed keeps its running module (a tenant's changed
    /// settings applied in place); one that does not launch is left out.
    /// The failure of each that did not, by id, in order.
    pub async fn launched(
        &self,
        processes: &[ProcessSpec],
        catalog: Option<&CatalogLauncher>,
    ) -> (Self, Vec<(String, HostError)>) {
        let mut kept = Vec::with_capacity(processes.len());
        let mut failed = Vec::new();
        for (spec, module) in
            crate::compose::launch::processes(processes, &self.processes, catalog).await
        {
            match module {
                Ok(module) => kept.push((spec, module)),
                Err(e) => failed.push((spec.id.clone(), e)),
            }
        }
        (
            self.with(self.hooks.clone(), kept, self.builtins.clone()),
            failed,
        )
    }

    /// These modules without the module process `id`: a hosted tenant goes
    /// on without a module that refuses its settings or does not start
    /// (post-cortex-cloud ARCHITECTURE, a tenant's modules).
    #[must_use]
    pub fn without(&self, id: &str) -> Self {
        let kept = self
            .processes
            .iter()
            .filter(|(spec, _)| spec.id != id)
            .cloned()
            .collect();
        self.with(self.hooks.clone(), kept, self.builtins.clone())
    }

    /// Whether a module process `id` is among them.
    #[must_use]
    pub fn runs(&self, id: &str) -> bool {
        self.processes.iter().any(|(spec, _)| spec.id == id)
    }

    /// These modules with the configured ones of `hooks`, `processes` and
    /// `builtins`: an entry that has not changed keeps its running module,
    /// the others are loaded (module processes serve their sockets in the
    /// system's temporary directory).
    ///
    /// # Errors
    /// An executable is not the approved one, or a process does not launch.
    pub async fn configured(
        &self,
        hooks: &[ScriptSpec],
        processes: &[ProcessSpec],
        builtins: &[BuiltinSpec],
    ) -> Result<Self, HostError> {
        let mut loaded_hooks = Vec::with_capacity(hooks.len());
        for spec in hooks {
            let module: Arc<dyn Module> = match self.hooks.iter().find(|(s, _)| s == spec) {
                Some((_, running)) => Arc::clone(running),
                None => Arc::new(ScriptModule::load(spec.clone())?),
            };
            loaded_hooks.push((spec.clone(), module));
        }
        let mut loaded_processes = Vec::with_capacity(processes.len());
        for (spec, module) in
            crate::compose::launch::processes(processes, &self.processes, None).await
        {
            loaded_processes.push((spec, module?));
        }
        let loaded_builtins = builtins
            .iter()
            .map(|spec| {
                let module = match self.builtins.iter().find(|(s, _)| s == spec) {
                    Some((_, running)) => Arc::clone(running),
                    None => spec.id.module(),
                };
                (spec.clone(), module)
            })
            .collect();
        Ok(self.with(loaded_hooks, loaded_processes, loaded_builtins))
    }

    /// These modules with `hooks`, `processes` and `builtins` in place of
    /// theirs.
    fn with(
        &self,
        hooks: Vec<(ScriptSpec, Arc<dyn Module>)>,
        processes: Vec<(ProcessSpec, Arc<dyn Module>)>,
        builtins: Vec<(BuiltinSpec, Arc<dyn Module>)>,
    ) -> Self {
        Self {
            memory: Arc::clone(&self.memory),
            kinds: Arc::clone(&self.kinds),
            store: Arc::clone(&self.store),
            sandbox: self.sandbox.clone(),
            deny: self.deny.clone(),
            hooks,
            processes,
            builtins,
            remote: self.remote.clone(),
            dynamic: self.dynamic.clone(),
            tenant: self.tenant.clone(),
        }
    }

    /// The running modules' document kinds, as memory reads them.
    #[must_use]
    pub fn kinds(&self) -> Arc<dyn DocumentKinds> {
        Arc::clone(&self.kinds)
    }

    /// Every module, memory first.
    #[must_use]
    pub fn all(&self) -> Vec<Arc<dyn Module>> {
        std::iter::once(Arc::clone(&self.memory))
            .chain(self.sandbox.iter().cloned())
            .chain(self.hooks.iter().map(|(_, m)| Arc::clone(m)))
            .chain(self.processes.iter().map(|(_, m)| Arc::clone(m)))
            .chain(self.builtins.iter().map(|(_, m)| Arc::clone(m)))
            .chain(self.remote.iter().map(Arc::clone))
            .chain(self.dynamic.iter().map(|(m, _)| Arc::clone(m)))
            .collect()
    }

    /// The configuration of each module that has one, by id.
    #[must_use]
    pub fn config(&self) -> HashMap<String, Value> {
        let processes = self
            .processes
            .iter()
            .filter_map(|(spec, _)| Some((spec.id.clone(), spec.config.clone()?)));
        let builtins = self
            .builtins
            .iter()
            .filter_map(|(spec, _)| Some((spec.id.id().to_string(), spec.config.clone()?)));
        let dynamic = self
            .dynamic
            .iter()
            .map(|(_, config)| (DynamicModule::ID.to_string(), config.clone()));
        processes.chain(builtins).chain(dynamic).collect()
    }

    /// What the operator grants each configured module, by id: the policy
    /// of its sandbox, from its entry's grants and filesystem roots, the
    /// daemon's denied names, and, for a tool module, that only this
    /// machine's people and agents call it.
    #[must_use]
    pub fn policies(&self) -> HashMap<String, Policy> {
        let processes = self.processes.iter().map(|(spec, _)| {
            (
                spec.id.clone(),
                sandbox_policy(&spec.grants, &spec.filesystem, &self.deny),
            )
        });
        let builtins = self.builtins.iter().map(|(spec, _)| {
            let policy = sandbox_policy(&spec.grants, &spec.filesystem, &self.deny);
            // The tool modules reach this machine's files and processes:
            // only its own people and agents call them (docs/connect.md 4).
            let local_only = matches!(
                spec.id,
                Builtin::ToolsFs | Builtin::ToolsShell | Builtin::ToolsGit
            );
            (
                spec.id.id().to_string(),
                Policy {
                    local_only,
                    ..policy
                },
            )
        });
        processes.chain(builtins).collect()
    }
}

/// The module host over `modules`: the standalone daemon (ARCHITECTURE
/// section 7).
///
/// # Errors
/// The host refused a module, or one failed to start.
pub async fn host(modules: &Modules) -> Result<Host, HostError> {
    // The organisation's events, kept by memory (docs/host.md, events of a
    // module).
    let events: Arc<dyn pcx_host::EventStore> =
        Arc::new(crate::compose::MemoryEvents::new(&modules.store));
    let host = Host::start_with(
        modules.all(),
        &modules.config(),
        &modules.policies(),
        Some(events),
        &modules.tenant,
    )
    .await?;
    // Documents written before W23 become their kinds' messages once the
    // modules that declare the kinds run; those that fail stay as they
    // are, and gRPC refuses them until a later start converts them.
    if let Err(e) = modules
        .store
        .documents
        .migrate(modules.kinds.as_ref())
        .await
    {
        tracing::error!(error = %e, "documents: the migration to messages failed");
    }
    Ok(host)
}
