// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Module processes launched at once: each launch (its binary's pin
//! checked, its process started, its manifest read) runs beside the others,
//! so a host with many modules starts in the time of its slowest, not the
//! sum of all; the outcome of each is kept apart, in order. A hosted
//! organisation's modules of the catalog launch through the launcher of the
//! pot `pcx-modules` (docs/modules/dynamic.md, a module of the catalog),
//! but `dynamic`, whose adapter is that launcher's client, so it runs beside
//! the host; the others directly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use pcx_dynamic::DynamicModule;
use pcx_host::{HostError, ProcessModule, ProcessSpec};
use pcx_module_api::Module;
use pcx_module_launcher::{Remote, Requested};

/// A module of the catalog's limits, its catalog entry's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Its memory, in MB.
    pub memory_mb: u32,
    /// Its open files.
    pub open_files: u32,
}

/// The launcher a hosted organisation's modules of the catalog run
/// through: each confined as the sandbox user, with only the environment
/// the launch names, and the network only where the launcher's own
/// configuration gives it (docs/host.md, network).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLauncher {
    /// The launcher's socket.
    pub socket: PathBuf,
    /// The directory of the launches' sockets, shared with it.
    pub sockets: PathBuf,
    /// Whose modules they are (the organisation's id): their scratch
    /// directories'.
    pub owner: String,
    /// Each module's limits, by id.
    pub limits: HashMap<String, Limits>,
}

impl CatalogLauncher {
    /// `spec`'s process, through the launcher, its executable checked
    /// there.
    async fn launch(&self, spec: &ProcessSpec) -> Result<ProcessModule, HostError> {
        let limits = self.limits.get(&spec.id).ok_or_else(|| HostError::Launch {
            module: spec.id.clone(),
            reason: "its catalog entry names no limits".to_string(),
        })?;
        let requested = Requested {
            owner: self.owner.clone(),
            read: Vec::new(),
            cwd: PathBuf::new(),
            env: HashMap::new(),
            memory_mb: limits.memory_mb,
            open_files: limits.open_files,
            sha256: spec.sha256.clone(),
        };
        ProcessModule::launch_by(
            spec,
            &self.sockets,
            Arc::new(Remote::new(self.socket.clone(), requested)),
        )
        .await
    }
}

/// Each of `specs` that is not `running` already, launched at once,
/// through `catalog` when there is one (`dynamic` directly); a running one
/// is kept as it is. In the order of `specs`.
pub async fn processes(
    specs: &[ProcessSpec],
    running: &[(ProcessSpec, Arc<dyn Module>)],
    catalog: Option<&CatalogLauncher>,
) -> Vec<(ProcessSpec, Result<Arc<dyn Module>, HostError>)> {
    let sockets = std::env::temp_dir();
    let launches = specs.iter().map(|spec| {
        let sockets = sockets.clone();
        async move {
            let module = match running.iter().find(|(s, _)| s == spec) {
                Some((_, module)) => Ok(Arc::clone(module)),
                None => match catalog.filter(|_| spec.id != DynamicModule::ID) {
                    Some(catalog) => catalog.launch(spec).await,
                    None => ProcessModule::launch(spec, &sockets).await,
                }
                .map(|m| Arc::new(m) as Arc<dyn Module>),
            };
            (spec.clone(), module)
        }
    });
    futures::future::join_all(launches).await
}
