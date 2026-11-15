// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The module in the `sandbox` slot: no tools, no events; it gives the host
//! a sandbox per policy.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use pcx_module_api::sandbox::{Policy, Sandbox, SandboxProvider};
use pcx_module_api::{
    API_VERSION, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind, ModuleKind,
    Outcome, Resource, Scope,
};

use crate::bound::Bound;
use crate::launcher::Launcher;

/// Gives a sandbox per module and policy.
struct Provider {
    launcher: Launcher,
    /// Where the modules' scratch directories are.
    scratch_root: PathBuf,
}

impl Provider {
    /// Empty every module's scratch directory, each staying where it is: a
    /// run starts with nothing of the last, and what another module writes
    /// there by its path before this one's first process stays.
    fn empty_scratch(&self) -> std::io::Result<()> {
        let dirs = match std::fs::read_dir(&self.scratch_root) {
            Ok(dirs) => dirs,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for dir in dirs {
            let dir = dir?.path();
            std::fs::remove_dir_all(&dir)?;
            std::fs::create_dir(&dir)?;
            self.launcher.give(&dir)?;
        }
        Ok(())
    }

    /// The scratch directory of `module`.
    fn scratch(&self, module: &str) -> Result<PathBuf, ModuleError> {
        let dir = self.scratch_root.join(module);
        let unavailable = |e: std::io::Error| {
            ModuleError::new(
                ModuleErrorKind::Unavailable,
                format!("scratch directory {}: {e}", dir.display()),
            )
        };
        std::fs::create_dir_all(&dir).map_err(unavailable)?;
        self.launcher.give(&dir).map_err(unavailable)?;
        Ok(dir)
    }
}

impl SandboxProvider for Provider {
    fn bind(&self, module: &str, policy: Policy) -> Result<Arc<dyn Sandbox>, ModuleError> {
        let scratch = if policy.spawn {
            Some(self.scratch(module)?)
        } else {
            None
        };
        Ok(Arc::new(Bound::new(
            policy,
            scratch,
            self.launcher.clone(),
        )?))
    }
}

/// The `pcx.sandbox` module.
pub struct SandboxModule {
    manifest: Manifest,
    provider: Arc<Provider>,
}

impl SandboxModule {
    /// The module's id.
    pub const ID: &'static str = "pcx.sandbox";

    /// The module confining processes with `launcher`, keeping the
    /// modules' scratch directories under `scratch_root`.
    #[must_use]
    pub fn new(launcher: Launcher, scratch_root: &Path) -> Self {
        Self {
            manifest: Manifest {
                description: "Runs a module’s files and processes within the roots, network and processes it is granted.".to_string(),
                id: Self::ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Sandbox.into(),
                ..Manifest::default()
            },
            provider: Arc::new(Provider {
                launcher,
                scratch_root: scratch_root.to_path_buf(),
            }),
        }
    }
}

#[async_trait]
impl Module for SandboxModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        self.provider.empty_scratch().map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::Unavailable,
                format!(
                    "scratch directories in {}: {e}",
                    self.provider.scratch_root.display()
                ),
            )
        })
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("the sandbox serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn sandbox_provider(&self) -> Option<Arc<dyn SandboxProvider>> {
        Some(self.provider.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_empties_each_scratch_directory_and_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let provider = Provider {
            launcher: Launcher::platform(None).unwrap(),
            scratch_root: root.path().to_path_buf(),
        };
        let dir = provider.scratch("tester").unwrap();
        std::fs::create_dir(dir.join("old")).unwrap();
        std::fs::write(dir.join("old/file"), b"from the last run").unwrap();

        provider.empty_scratch().unwrap();

        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::write(dir.join("new"), b"written by another module").unwrap();
        assert_eq!(provider.scratch("tester").unwrap(), dir);
        assert!(dir.join("new").exists(), "a first use empties nothing");
    }

    #[test]
    fn no_scratch_directories_yet_is_nothing_to_empty() {
        let root = tempfile::tempdir().unwrap();
        let provider = Provider {
            launcher: Launcher::platform(None).unwrap(),
            scratch_root: root.path().join("none"),
        };
        provider.empty_scratch().unwrap();
    }
}
