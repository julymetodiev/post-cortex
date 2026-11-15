// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Changing a running module's configuration (ARCHITECTURE section 7,
//! Module UI, settings): the change is checked against the module's schema,
//! its secrets kept or removed by the public rules, and handed to the
//! host's [`Settings`] port, whose adapter persists it and reloads the
//! module (the daemon's writes `daemon.toml`); so is a change of what a
//! module's sandbox lets it use ([`ModuleGrants`]), which turns on a
//! built-in module that does not run yet. A host without the port (a
//! hosted tenant, a connected daemon's routes through the cloud) refuses
//! both.

use std::sync::Arc;

use async_trait::async_trait;
use pbjson_types::Any;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use serde_json::Value;

use crate::{Host, HostError, ModuleGrants};

/// Where a host's module configurations live: persists one and reloads its
/// module with it.
#[async_trait]
pub trait Settings: Send + Sync {
    /// Make `config` module `id`'s configuration and reload the module with
    /// it; on failure the previous configuration stays, persisted and
    /// running.
    ///
    /// # Errors
    /// See [`SettingsError`].
    async fn apply(&self, id: &str, config: Value) -> Result<(), SettingsError>;

    /// Give module `id` `grants` (its other grants stay) and reload it with
    /// them, turning it on when it is a built-in module that does not run;
    /// on failure the previous grants stay, persisted and running.
    ///
    /// # Errors
    /// See [`SettingsError`].
    async fn grant(&self, id: &str, grants: ModuleGrants) -> Result<(), SettingsError>;

    /// The built-in modules [`Settings::grant`] can turn on that do not run.
    fn available(&self) -> Vec<String>;
}

/// Why a configuration change was not applied.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The module's configuration is not kept where the port keeps them (a
    /// built-in module, a hook script).
    #[error("module {module} is not configured here")]
    NotConfigurable {
        /// The module.
        module: String,
    },
    /// No module of that id runs or can be turned on here.
    #[error("no module {module} runs or can be turned on here")]
    Unknown {
        /// The module.
        module: String,
    },
    /// A root of a grant is not a directory a module can be given.
    #[error("root {path}: {reason}")]
    Root {
        /// The root, as asked.
        path: String,
        /// Why.
        reason: String,
    },
    /// The store cannot hold the configuration (TOML has no `null`).
    #[error("{reason}")]
    Unrepresentable {
        /// What it cannot hold.
        reason: String,
    },
    /// The store could not be read or written.
    #[error("the module settings could not be saved: {reason}")]
    Store {
        /// Why.
        reason: String,
    },
    /// The module did not reload with the configuration.
    #[error(transparent)]
    Reload(#[from] HostError),
}

impl From<SettingsError> for ModuleError {
    fn from(e: SettingsError) -> Self {
        let kind = match &e {
            SettingsError::NotConfigurable { .. } => ModuleErrorKind::FailedPrecondition,
            SettingsError::Unknown { .. } => ModuleErrorKind::NotFound,
            SettingsError::Root { .. }
            | SettingsError::Unrepresentable { .. }
            | SettingsError::Reload(HostError::InvalidConfig { .. }) => {
                ModuleErrorKind::InvalidArgument
            }
            SettingsError::Store { .. } => ModuleErrorKind::Internal,
            SettingsError::Reload(_) => ModuleErrorKind::Unavailable,
        };
        match e {
            SettingsError::Reload(HostError::InvalidConfig { error, .. }) => error.into(),
            e => ModuleError::new(kind, e.to_string()),
        }
    }
}

/// Changes the configurations of a host's running modules.
pub struct ModuleSettings {
    host: Arc<Host>,
    store: Option<Arc<dyn Settings>>,
}

impl ModuleSettings {
    /// Changes to `host`'s modules, kept by `store`; none: the host refuses
    /// changes.
    #[must_use]
    pub fn new(host: Arc<Host>, store: Option<Arc<dyn Settings>>) -> Self {
        Self { host, store }
    }

    /// Make `change` module `id`'s configuration: a secret it leaves unset
    /// keeps its value, one `clear` names is removed, and the result must
    /// fit the module's message.
    ///
    /// # Errors
    /// `FailedPrecondition` when the host does not change configurations,
    /// not this module's, or the module takes none; `NotFound` for a module
    /// that does not run; `InvalidArgument` naming the setting at fault;
    /// `Unavailable` when the module did not start with it (the previous
    /// one stays); `Internal` when it could not be saved.
    pub async fn configure(
        &self,
        id: &str,
        change: &Any,
        clear: &[String],
    ) -> Result<(), ModuleError> {
        let store = self.store()?;
        let current = self.host.configured(id).ok_or_else(|| {
            ModuleError::new(ModuleErrorKind::NotFound, format!("no module {id}"))
        })?;
        let schema = current.schema.as_ref().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("module {id} takes no configuration"),
            )
        })?;
        let config = schema.keep_secrets(change, clear, &current.config)?;
        schema.check(&config)?;
        // The store keeps it as a configuration file does: its JSON form.
        let config = schema.to_json(&config)?;
        Ok(store.apply(id, config).await?)
    }

    /// Give module `id` `grants`: each root an absolute path (the store
    /// checks it is a directory), its other grants as they are.
    ///
    /// # Errors
    /// `FailedPrecondition` when the host does not grant through it;
    /// `NotFound` for a module that neither runs nor can be turned on;
    /// `InvalidArgument` naming the root at fault; `Unavailable` when the
    /// module did not start with them (the previous ones stay); `Internal`
    /// when they could not be saved.
    pub async fn grant(&self, id: &str, grants: ModuleGrants) -> Result<(), ModuleError> {
        let store = self.store()?;
        if let Some(root) = grants.roots.iter().find(|r| !r.path.is_absolute()) {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("root {}: not an absolute path", root.path.display()),
            ));
        }
        Ok(store.grant(id, grants).await?)
    }

    /// The built-in modules the host can turn on that do not run; none
    /// where it does not grant through it.
    #[must_use]
    pub fn available(&self) -> Vec<String> {
        self.store.as_ref().map_or_else(Vec::new, |s| s.available())
    }

    /// The store, when the host changes modules through it.
    fn store(&self) -> Result<&Arc<dyn Settings>, ModuleError> {
        self.store.as_ref().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "this host's modules are not configured or granted through it",
            )
        })
    }
}
