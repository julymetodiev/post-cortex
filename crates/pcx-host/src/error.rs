// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Why the host refused or could not start its modules.

use std::path::PathBuf;

use pcx_module_api::view::ViewError;
use pcx_module_api::{ApiVersion, ConfigError, ModuleError};

impl HostError {
    /// The module whose own configuration or start is at fault: one a host
    /// may go on without (a hosted tenant leaves it out), or whose new
    /// settings it does not apply. None for what is wrong between modules
    /// or with how a module is run.
    #[must_use]
    pub fn refused_module(&self) -> Option<&str> {
        match self {
            Self::InvalidConfig { module, .. } | Self::StartFailed { module, .. } => Some(module),
            _ => None,
        }
    }
}

/// A module the host refuses, or one that could not start.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The module speaks a module API the host does not.
    #[error(
        "module {module} speaks API {}.{}, the host {}.{}",
        module_api.major, module_api.minor, host_api.major, host_api.minor
    )]
    IncompatibleApi {
        /// The module.
        module: String,
        /// Its API version.
        module_api: ApiVersion,
        /// The host's.
        host_api: ApiVersion,
    },
    /// The manifest is malformed (no id, no kind, an unset event).
    #[error("module {module}: {reason}")]
    InvalidManifest {
        /// The module.
        module: String,
        /// What is wrong.
        reason: String,
    },
    /// Two modules share an id.
    #[error("two modules have the id {0}")]
    DuplicateId(String),
    /// An exclusive slot (memory, context engine) is taken.
    #[error("slot {slot} is taken by {existing}; {module} cannot load")]
    SlotTaken {
        /// The slot.
        slot: &'static str,
        /// The module holding it.
        existing: String,
        /// The module refused.
        module: String,
    },
    /// Two modules offer a tool of one name.
    #[error("tool {tool} is offered by {first} and {module}")]
    DuplicateTool {
        /// The tool.
        tool: String,
        /// The module that offered it first.
        first: String,
        /// The module refused.
        module: String,
    },
    /// Two modules name a tool on one RPC's path: a workflow's block and
    /// the catalogs name a tool by its path (docs/host.md, a service
    /// several modules serve).
    #[error("RPC {path} is a tool of {first} and {module}")]
    DuplicateToolPath {
        /// The RPC's gRPC path.
        path: String,
        /// The module that names a tool on it first.
        first: String,
        /// The module refused.
        module: String,
    },
    /// A provider lists a model out of shape.
    #[error("module {module}: {source}")]
    InvalidModel {
        /// The provider.
        module: String,
        /// What is wrong.
        source: pcx_module_api::model::ModelInfoError,
    },
    /// Two providers serve a model of one id.
    #[error("model {model} is served by {first} and {module}")]
    DuplicateModel {
        /// The model.
        model: String,
        /// The provider that served it first.
        first: String,
        /// The provider refused.
        module: String,
    },
    /// A module's executable cannot be read.
    #[error("module {module}: cannot read {}: {source}", path.display())]
    ExecutableUnreadable {
        /// The module.
        module: String,
        /// The executable.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// A module's executable is not the approved one.
    #[error(
        "module {module}: {} has sha256 {found}, not the approved one; approve it in the configuration",
        path.display()
    )]
    ExecutableNotApproved {
        /// The module.
        module: String,
        /// The executable.
        path: PathBuf,
        /// Its hash now.
        found: String,
    },
    /// A module asks for a capability the configuration does not grant it.
    #[error(
        "module {module} asks for {capability}, which is not granted; grant it in the configuration"
    )]
    NotGranted {
        /// The module.
        module: String,
        /// The capability.
        capability: String,
    },
    /// A module process uses a module the configuration does not grant.
    #[error(
        "module {module} uses module {uses}, which is not granted; add it to its uses in the configuration"
    )]
    UseNotGranted {
        /// The module.
        module: String,
        /// The module it uses.
        uses: String,
    },
    /// A module process could not be launched or reached.
    #[error("module {module}: {reason}")]
    Launch {
        /// The module.
        module: String,
        /// Why.
        reason: String,
    },
    /// A module's configuration does not fit its schema; it is not
    /// started.
    #[error("module {module}: {error}")]
    InvalidConfig {
        /// The module.
        module: String,
        /// The setting at fault and why.
        error: ConfigError,
    },
    /// The module failed to start.
    #[error("module {module} failed to start: {error}")]
    StartFailed {
        /// The module.
        module: String,
        /// Why.
        error: ModuleError,
    },
    /// A view of the module does not fit its tools (ARCHITECTURE section
    /// 7, Module UI).
    #[error("module {module}: {error}")]
    InvalidView {
        /// The module.
        module: String,
        /// The view at fault and why.
        error: ViewError,
    },
}
