// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a module in another language declares, read from its plain values:
//! its RPCs, events and settings, which make its manifest.

use post_cortex_proto::module::EmittedEvent;

use crate::{
    API_VERSION, Effect, Manifest, ModuleError, ModuleErrorKind, ModuleKind, Rpc, Visibility,
};

/// The effect a language names: `read`, `write` or `external`.
///
/// # Errors
/// `InvalidArgument` for another word.
pub fn effect(name: &str) -> Result<Effect, ModuleError> {
    match name {
        "read" => Ok(Effect::Read),
        "write" => Ok(Effect::Write),
        "external" => Ok(Effect::External),
        other => Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("{other} is no effect: read, write or external"),
        )),
    }
}

/// The visibility a language names: `both`, `model` or `app`.
///
/// # Errors
/// `InvalidArgument` for another word.
pub fn visibility(name: &str) -> Result<Visibility, ModuleError> {
    match name {
        "both" => Ok(Visibility::Both),
        "model" => Ok(Visibility::Model),
        "app" => Ok(Visibility::App),
        other => Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("{other} is no visibility: both, model or app"),
        )),
    }
}

/// The failure a language's handler gives: its kind's name (as
/// [`ModuleErrorKind::name`]) and its message; an unknown kind is
/// `internal`, its name kept in the message.
#[must_use]
pub fn failure(kind: &str, message: String) -> ModuleError {
    match ModuleErrorKind::named(kind) {
        Some(kind) => ModuleError::new(kind, message),
        None => ModuleError::new(
            ModuleErrorKind::Internal,
            format!("{message} (a failure of unknown kind {kind})"),
        ),
    }
}

/// An RPC of the module's own services, as the language declares it.
#[derive(Debug, Clone)]
pub struct Declared {
    /// Its method's path, `/<package>.<Service>/<Method>`.
    pub method: String,
    /// Its tool's name (empty: not a tool).
    pub tool: String,
    /// What a call of it does.
    pub effect: Effect,
    /// Who may call the tool.
    pub visibility: Visibility,
}

/// An event the module emits.
#[derive(Debug, Clone)]
pub struct Emitted {
    /// Its name, after the module's id.
    pub name: String,
    /// Its payload's message type.
    pub message: String,
    /// What it means.
    pub description: String,
}

/// What a module in another language declares; the core makes its
/// manifest, which the host checks as any other.
#[derive(Debug, Clone, Default)]
pub struct Declaration {
    /// Its id.
    pub id: String,
    /// Its version.
    pub version: String,
    /// What it does.
    pub description: String,
    /// Its gRPC services, `<package>.<Service>`.
    pub services: Vec<String>,
    /// The `FileDescriptorSet` of its proto.
    pub descriptors: Vec<u8>,
    /// Its RPCs.
    pub rpcs: Vec<Declared>,
    /// The modules it uses.
    pub uses: Vec<String>,
    /// The events it emits.
    pub emits: Vec<Emitted>,
    /// The events it follows.
    pub follows: Vec<String>,
    /// Its configuration's message type (empty: it takes none).
    pub config_type: String,
}

/// The manifest of what was declared, which the host checks as any
/// other.
impl From<Declaration> for Manifest {
    fn from(declaration: Declaration) -> Self {
        Manifest {
            id: declaration.id,
            version: declaration.version,
            description: declaration.description,
            api: Some(API_VERSION),
            kind: ModuleKind::Tool.into(),
            services: declaration.services,
            descriptors: declaration.descriptors,
            uses: declaration.uses,
            follows: declaration.follows,
            config_type: declaration.config_type,
            rpcs: declaration
                .rpcs
                .into_iter()
                .map(|r| Rpc {
                    method: r.method,
                    tool: r.tool,
                    visibility: r.visibility.to_wire(),
                    effect: r.effect.into(),
                })
                .collect(),
            emits: declaration
                .emits
                .into_iter()
                .map(|e| EmittedEvent {
                    name: e.name,
                    r#type: e.message,
                    description: e.description,
                })
                .collect(),
            ..Manifest::default()
        }
    }
}
