// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Why a module call failed: the kinds a host or caller acts on.

use post_cortex_proto::module as wire;
use serde_json::Value;

/// The kind of a [`ModuleError`] (the `pcx.module.v1` `ModuleErrorKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleErrorKind {
    /// The input is malformed.
    InvalidArgument,
    /// Something the input names does not exist.
    NotFound,
    /// The module or a backend it needs is unavailable; retrying may work.
    Unavailable,
    /// The deadline passed.
    DeadlineExceeded,
    /// A capability the call needs is not granted.
    Denied,
    /// A defect of the module.
    Internal,
    /// The change conflicts with the current state.
    Conflict,
    /// The state does not allow the call yet.
    FailedPrecondition,
    /// A limit is spent (a daily budget); it is reset later.
    ResourceExhausted,
    /// The module runs elsewhere (a runner) and that place is not
    /// connected: it comes back by itself, so a caller waits rather than
    /// tries again (docs/connect.md section 7).
    Offline,
}

impl ModuleErrorKind {
    /// Its name in snake case (`invalid_argument`, `denied`): how a
    /// workflow's step and a module's log write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Denied => "denied",
            Self::Internal => "internal",
            Self::Conflict => "conflict",
            Self::FailedPrecondition => "failed_precondition",
            Self::ResourceExhausted => "resource_exhausted",
            Self::Offline => "offline",
        }
    }

    /// The kind of `name`, as [`ModuleErrorKind::name`] writes it; none
    /// for another word.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        [
            Self::InvalidArgument,
            Self::NotFound,
            Self::Unavailable,
            Self::DeadlineExceeded,
            Self::Denied,
            Self::Internal,
            Self::Conflict,
            Self::FailedPrecondition,
            Self::ResourceExhausted,
            Self::Offline,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// The metadata key that marks an `Unavailable` gRPC status as
/// [`ModuleErrorKind::Offline`] (gRPC has no code of its own for it).
pub const OFFLINE_METADATA: &str = "x-pcx-offline";

/// A failed module call.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ModuleError {
    /// What a caller acts on.
    pub kind: ModuleErrorKind,
    /// What went wrong, for a person.
    pub message: String,
    /// The setting or argument at fault (`token`, `slack.channel`); none
    /// in particular.
    pub field: Option<String>,
    /// Structured details an MCP client acts on (the argument at fault, a
    /// hint, suggestions; a bulk write's per-item results): memory's MCP
    /// tools'; they do not cross the wire.
    pub details: Option<Value>,
}

impl ModuleError {
    /// An error of `kind`, without details.
    pub fn new(kind: ModuleErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            field: None,
            details: None,
        }
    }

    /// The error naming `field` as the one at fault.
    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// The error with `details`.
    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl From<ModuleError> for wire::ModuleError {
    fn from(e: ModuleError) -> Self {
        let kind = match e.kind {
            ModuleErrorKind::InvalidArgument => wire::ModuleErrorKind::InvalidArgument,
            ModuleErrorKind::NotFound => wire::ModuleErrorKind::NotFound,
            ModuleErrorKind::Unavailable => wire::ModuleErrorKind::Unavailable,
            ModuleErrorKind::DeadlineExceeded => wire::ModuleErrorKind::DeadlineExceeded,
            ModuleErrorKind::Denied => wire::ModuleErrorKind::Denied,
            ModuleErrorKind::Internal => wire::ModuleErrorKind::Internal,
            ModuleErrorKind::Conflict => wire::ModuleErrorKind::Conflict,
            ModuleErrorKind::FailedPrecondition => wire::ModuleErrorKind::FailedPrecondition,
            ModuleErrorKind::ResourceExhausted => wire::ModuleErrorKind::ResourceExhausted,
            ModuleErrorKind::Offline => wire::ModuleErrorKind::Offline,
        };
        Self {
            kind: kind.into(),
            message: e.message,
            field: e.field.unwrap_or_default(),
        }
    }
}

impl From<wire::ModuleError> for ModuleError {
    /// An unknown or unset kind is `Internal`: the module sent no kind a
    /// caller can act on.
    fn from(e: wire::ModuleError) -> Self {
        let kind = match wire::ModuleErrorKind::try_from(e.kind) {
            Ok(wire::ModuleErrorKind::InvalidArgument) => ModuleErrorKind::InvalidArgument,
            Ok(wire::ModuleErrorKind::NotFound) => ModuleErrorKind::NotFound,
            Ok(wire::ModuleErrorKind::Unavailable) => ModuleErrorKind::Unavailable,
            Ok(wire::ModuleErrorKind::DeadlineExceeded) => ModuleErrorKind::DeadlineExceeded,
            Ok(wire::ModuleErrorKind::Denied) => ModuleErrorKind::Denied,
            Ok(wire::ModuleErrorKind::Conflict) => ModuleErrorKind::Conflict,
            Ok(wire::ModuleErrorKind::FailedPrecondition) => ModuleErrorKind::FailedPrecondition,
            Ok(wire::ModuleErrorKind::ResourceExhausted) => ModuleErrorKind::ResourceExhausted,
            Ok(wire::ModuleErrorKind::Offline) => ModuleErrorKind::Offline,
            Ok(wire::ModuleErrorKind::Internal | wire::ModuleErrorKind::Unspecified) | Err(_) => {
                ModuleErrorKind::Internal
            }
        };
        Self {
            kind,
            message: e.message,
            field: Some(e.field).filter(|f| !f.is_empty()),
            details: None,
        }
    }
}

impl From<tonic::Status> for ModuleError {
    /// The error of a failed `pcx.module.v1` RPC, by its status code (an
    /// `Unavailable` one marked [`OFFLINE_METADATA`] is `Offline`).
    fn from(status: tonic::Status) -> Self {
        use tonic::Code;
        let kind = match status.code() {
            Code::Unavailable if status.metadata().contains_key(OFFLINE_METADATA) => {
                ModuleErrorKind::Offline
            }
            Code::InvalidArgument | Code::OutOfRange => ModuleErrorKind::InvalidArgument,
            Code::NotFound => ModuleErrorKind::NotFound,
            Code::DeadlineExceeded | Code::Cancelled => ModuleErrorKind::DeadlineExceeded,
            Code::PermissionDenied | Code::Unauthenticated => ModuleErrorKind::Denied,
            Code::Unavailable => ModuleErrorKind::Unavailable,
            Code::ResourceExhausted => ModuleErrorKind::ResourceExhausted,
            Code::AlreadyExists | Code::Aborted => ModuleErrorKind::Conflict,
            Code::FailedPrecondition => ModuleErrorKind::FailedPrecondition,
            _ => ModuleErrorKind::Internal,
        };
        ModuleError::new(kind, status.message())
    }
}

impl From<ModuleError> for tonic::Status {
    /// The status of an RPC that fails with `error`: the inverse of the
    /// mapping above.
    fn from(error: ModuleError) -> Self {
        use tonic::Code;
        let code = match error.kind {
            ModuleErrorKind::InvalidArgument => Code::InvalidArgument,
            ModuleErrorKind::NotFound => Code::NotFound,
            ModuleErrorKind::Unavailable => Code::Unavailable,
            ModuleErrorKind::DeadlineExceeded => Code::DeadlineExceeded,
            ModuleErrorKind::Denied => Code::PermissionDenied,
            ModuleErrorKind::Internal => Code::Internal,
            ModuleErrorKind::Conflict => Code::Aborted,
            ModuleErrorKind::FailedPrecondition => Code::FailedPrecondition,
            ModuleErrorKind::ResourceExhausted => Code::ResourceExhausted,
            ModuleErrorKind::Offline => Code::Unavailable,
        };
        let mut status = tonic::Status::new(code, error.message);
        if error.kind == ModuleErrorKind::Offline {
            status.metadata_mut().insert(
                OFFLINE_METADATA,
                tonic::metadata::MetadataValue::from_static("1"),
            );
        }
        status
    }
}
