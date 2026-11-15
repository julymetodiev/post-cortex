// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a running module was started with, and what the host's clients see
//! of it (`ListModules`): its configuration without secrets, its health and
//! its sandbox.

use std::time::Duration;

use pbjson_types::Any;
use pcx_module_api::agent::TurnGrant;
use pcx_module_api::sandbox::Policy;
use pcx_module_api::{ConfigError, ConfigType, Manifest, ModuleError};
use post_cortex_proto::module as wire;
use serde_json::Value;

use crate::ModuleHealth;

/// The configuration a module was started with, and its schema.
#[derive(Debug, Clone)]
pub struct Configured {
    /// The message of its configuration, when it takes one.
    pub schema: Option<ConfigType>,
    /// The configuration it runs with (empty: none).
    pub config: Any,
    /// The configuration without its secrets, and the secrets it sets.
    pub shown: (Any, Vec<String>),
    /// What the operator grants it: the policy of its sandbox.
    pub policy: Policy,
    /// What its settings give the turns it asks of the agent.
    pub grant: TurnGrant,
    /// The persons its settings let it act for (W29).
    pub persons: Vec<String>,
}

/// A running module as the host's clients see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleInfo {
    /// What it is.
    pub manifest: Manifest,
    /// Its configuration without its secrets (empty: none).
    pub config: Any,
    /// The secrets set, as paths into the configuration.
    pub secrets_set: Vec<String>,
    /// How it fares.
    pub health: ModuleHealth,
    /// What its sandbox lets it use, as the operator granted it.
    pub sandbox: Policy,
    /// The packages of the messages it defines, when a list shows it.
    pub packages: Vec<String>,
}

impl ModuleInfo {
    /// The module as a list shows it: without its descriptors, the bulk of
    /// a manifest (one module's, whole, is asked for by its id).
    #[must_use]
    pub fn listed(mut self) -> Self {
        match prost_reflect::DescriptorPool::decode(self.manifest.descriptors.as_slice()) {
            Ok(pool) => {
                self.packages = pool.files().map(|f| f.package_name().to_string()).collect();
                self.packages.sort();
                self.packages.dedup();
            }
            // Its client looks through every module's descriptors instead.
            Err(e) => {
                tracing::warn!(module = %self.manifest.id, error = %e, "a module's descriptors do not decode");
            }
        }
        self.manifest.descriptors.clear();
        self
    }
}

impl Configured {
    /// `manifest`'s module as its clients see it.
    pub fn info(&self, manifest: &Manifest, health: ModuleHealth) -> ModuleInfo {
        let (config, secrets_set) = self.shown.clone();
        ModuleInfo {
            manifest: manifest.clone(),
            config,
            secrets_set,
            health,
            sandbox: self.policy.clone(),
            packages: Vec::new(),
        }
    }
}

/// A configuration as a module runs with it.
pub struct Reading {
    /// The configuration.
    pub config: Any,
    /// It without its secrets, and the secrets it sets.
    pub shown: (Any, Vec<String>),
    /// The grant of the module's turns.
    pub grant: TurnGrant,
    /// The persons it acts for.
    pub persons: Vec<String>,
}

/// `config`, a configuration of `schema` (none: the module takes none), as
/// the module runs with it.
///
/// # Errors
/// It is not one of the message.
pub fn reading(schema: Option<&ConfigType>, config: Any) -> Result<Reading, ConfigError> {
    match schema {
        Some(schema) => Ok(Reading {
            shown: schema.redact(&config)?,
            grant: schema.grant(&config)?,
            persons: schema.persons(&config)?,
            config,
        }),
        None => Ok(Reading {
            config: Any::default(),
            shown: (Any::default(), Vec::new()),
            grant: TurnGrant::default(),
            persons: Vec::new(),
        }),
    }
}

/// The configuration a module of `schema` runs with from `section`, its
/// part of the host's configuration (its JSON form): read as its message
/// and checked; a module that takes none takes no section.
///
/// # Errors
/// The section is not one of its message, or set for a module that takes
/// none.
pub fn read(schema: Option<&ConfigType>, section: &Value) -> Result<Reading, ConfigError> {
    match schema {
        Some(config) => {
            let read = config.from_json(section)?;
            config.check(&read)?;
            reading(schema, read)
        }
        None if section.is_null() || section.as_object().is_some_and(|s| s.is_empty()) => {
            reading(None, Any::default())
        }
        None => Err(ConfigError {
            field: String::new(),
            reason: "is set, but the module takes none".to_string(),
        }),
    }
}

impl TryFrom<ModuleInfo> for wire::ModuleInfo {
    type Error = ModuleError;

    /// # Errors
    /// A root of its sandbox is not UTF-8.
    fn try_from(info: ModuleInfo) -> Result<Self, ModuleError> {
        Ok(Self {
            sandbox: Some(info.sandbox.to_wire()?),
            manifest: Some(info.manifest),
            config: Some(info.config).filter(|c| !c.type_url.is_empty()),
            secrets_set: info.secrets_set,
            health: Some(info.health.into()),
            packages: info.packages,
        })
    }
}

impl From<ModuleHealth> for wire::ModuleHealth {
    fn from(health: ModuleHealth) -> Self {
        Self {
            calls: health.calls,
            errors: health.errors,
            timeouts: health.timeouts,
            panics: health.panics,
            cancelled: health.cancelled,
            refused: health.refused,
            breaker_open: health.breaker_open,
            p50_ms: health.p50.map(millis),
            p99_ms: health.p99.map(millis),
            problem: health.problem,
        }
    }
}

/// `latency` in whole milliseconds; past the last bucket (`Duration::MAX`)
/// it is `u64::MAX`.
fn millis(latency: Duration) -> u64 {
    u64::try_from(latency.as_millis()).unwrap_or(u64::MAX)
}
