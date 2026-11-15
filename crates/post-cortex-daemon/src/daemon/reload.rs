// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Hot reload of the configured modules: when `daemon.toml` changes, the
//! host gets the new set of hook scripts and module processes without a
//! restart. A module whose spec did not change keeps running; a
//! configuration that does not load, or a module that does not start,
//! leaves the running set as it is. One [`Reloader`] holds the running set:
//! the file watcher and the host's settings port (a module configuration
//! changed, or its sandbox granted, through `pcx.module.v1.Host`, written
//! into `daemon.toml`) both reload through it, so a change reloads once.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use pcx_host::{Host, ModuleGrants, Settings, SettingsError};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::compose::{Builtin, Modules};
use crate::daemon::DaemonConfig;
use crate::daemon::{settings_file, settings_grants};

/// How often the configuration file is looked at.
const POLL: Duration = Duration::from_secs(2);

/// The daemon's configured modules, reloaded from `daemon.toml` one change
/// at a time.
pub struct Reloader {
    host: Arc<Host>,
    /// The configuration file.
    path: PathBuf,
    /// The running set; held across a reload.
    modules: Mutex<Modules>,
}

impl Reloader {
    /// The reloader of `host`, running `modules`, from the configuration
    /// file at `path`.
    pub fn new(host: Arc<Host>, modules: Modules, path: PathBuf) -> Self {
        Self {
            host,
            path,
            modules: Mutex::new(modules),
        }
    }

    /// Swap in the modules the configuration names now; false when they are
    /// the running ones.
    ///
    /// # Errors
    /// The configuration does not load, or the host refused the new set
    /// (the running set stays).
    pub async fn reload(&self) -> Result<bool, SettingsError> {
        let mut modules = self.modules.lock().await;
        reload_locked(&self.host, &self.path, &mut modules).await
    }

    /// Rewrite `daemon.toml` by `edit` and reload; when the reload fails,
    /// the previous file is put back.
    async fn rewrite(
        &self,
        edit: impl FnOnce(&str) -> Result<String, SettingsError>,
    ) -> Result<(), SettingsError> {
        let mut modules = self.modules.lock().await;
        let path = &self.path;
        let before = settings_file::read(path)?;
        settings_file::write(path, &edit(&before)?)?;
        match reload_locked(&self.host, path, &mut modules).await {
            Ok(_) => Ok(()),
            Err(e) => {
                if let Err(restore) = settings_file::write(path, &before) {
                    warn!(
                        "{} not restored after a refused change: {restore}",
                        path.display()
                    );
                }
                Err(e)
            }
        }
    }
}

#[async_trait]
impl Settings for Reloader {
    /// Write module `id`'s `config` in `daemon.toml` (only that) and reload.
    async fn apply(&self, id: &str, config: Value) -> Result<(), SettingsError> {
        self.rewrite(|text| settings_file::with_config(text, id, &config))
            .await?;
        info!("Module {id} configured in {}", self.path.display());
        Ok(())
    }

    /// Check each root is a directory, write module `id`'s grants in
    /// `daemon.toml` (a built-in it does not name added) and reload.
    async fn grant(&self, id: &str, grants: ModuleGrants) -> Result<(), SettingsError> {
        for root in &grants.roots {
            let refused = |reason: String| SettingsError::Root {
                path: root.path.display().to_string(),
                reason,
            };
            match std::fs::metadata(&root.path) {
                Ok(found) if found.is_dir() => {}
                Ok(_) => return Err(refused("not a directory".to_string())),
                Err(e) => return Err(refused(e.to_string())),
            }
        }
        let builtin = Builtin::of(id).is_some();
        self.rewrite(|text| settings_grants::with_grants(text, id, &grants, builtin))
            .await?;
        info!("Module {id} granted in {}", self.path.display());
        Ok(())
    }

    /// The built-in modules that do not run.
    fn available(&self) -> Vec<String> {
        Builtin::ALL
            .into_iter()
            .map(Builtin::id)
            .filter(|id| self.host.module(id).is_none())
            .map(String::from)
            .collect()
    }
}

/// Reload through `reloader` whenever the configuration file changes, until
/// `ct` is cancelled.
pub async fn watch(reloader: Arc<Reloader>, ct: CancellationToken) {
    let path = reloader.path.clone();
    let mut seen = modified(&path);
    loop {
        tokio::select! {
            () = ct.cancelled() => return,
            () = tokio::time::sleep(POLL) => {}
        }
        let now = modified(&path);
        if now == seen {
            continue;
        }
        seen = now;
        match reloader.reload().await {
            Ok(true) => info!("Modules reloaded from {}", path.display()),
            Ok(false) => {}
            Err(e) => warn!("Modules not reloaded from {}: {e}", path.display()),
        }
    }
}

/// Swap in the modules the configuration at `path` names now, `modules`
/// being the running ones; false when they are the same.
async fn reload_locked(
    host: &Host,
    path: &Path,
    modules: &mut Modules,
) -> Result<bool, SettingsError> {
    let config = DaemonConfig::load_from(path).map_err(|e| SettingsError::Store {
        reason: e.to_string(),
    })?;
    if modules.are(&config.hooks, &config.modules, &config.builtins) {
        return Ok(false);
    }
    let next = modules
        .configured(&config.hooks, &config.modules, &config.builtins)
        .await?;
    host.reload(next.all(), &next.config(), &next.policies())
        .await?;
    *modules = next;
    Ok(true)
}

/// When `path` last changed; none while it does not exist.
fn modified(path: &Path) -> Option<SystemTime> {
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(time) => Some(time),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!("Cannot read {}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests;
