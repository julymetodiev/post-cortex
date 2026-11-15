// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The one classification of failed operations (ARCHITECTURE rule 5).
//!
//! Every capability error converts into a [`Failure`] here, and nowhere
//! else; each transport maps a failure's [`Kind`] once (MCP tool error,
//! gRPC status, HTTP status), so the same error is the same kind on every
//! transport.

use crate::capabilities::{
    AdminError, AssemblyError, BlobError, CardError, DocumentError, EntityError, FreshnessError,
    MessageError, ProjectError, RecordError, ScopeError, SearchError, SessionError, TransferError,
    ViewsError, WorkspaceError,
};
use crate::{PersistError, RuntimeError};
use post_cortex_core::core::wire::UnknownVariant;
use post_cortex_ports::{KindsError, PortError};
use serde::Serialize;

/// What kind of failure an operation had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The request is malformed or names an invalid value.
    InvalidArgument,
    /// A session, workspace, entity or record it names does not exist.
    NotFound,
    /// It conflicts with what is stored (a taken name).
    Conflict,
    /// The caller may not do it (a document of another module's kind).
    Denied,
    /// The store is not in the state the operation needs (another
    /// embedding model's vectors).
    FailedPrecondition,
    /// Refused for now (the write circuit breaker is open); retry later.
    Unavailable,
    /// Storage or another internal failure.
    Internal,
}

impl Kind {
    /// Every kind.
    pub const ALL: [Kind; 7] = [
        Kind::InvalidArgument,
        Kind::NotFound,
        Kind::Conflict,
        Kind::Denied,
        Kind::FailedPrecondition,
        Kind::Unavailable,
        Kind::Internal,
    ];

    /// Wire name (`not_found`), as MCP error details carry it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::InvalidArgument => "invalid_argument",
            Kind::NotFound => "not_found",
            Kind::Conflict => "conflict",
            Kind::Denied => "denied",
            Kind::FailedPrecondition => "failed_precondition",
            Kind::Unavailable => "unavailable",
            Kind::Internal => "internal",
        }
    }
}

/// Details of an invalid argument, for the caller to fix its request.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ArgumentDetails {
    /// The argument at fault (`session_id`, `updates[2].supersedes`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter: Option<String>,
    /// What it must be.
    #[serde(rename = "expectedType", skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// What it was.
    #[serde(rename = "receivedValue", skip_serializing_if = "Option::is_none")]
    pub received: Option<serde_json::Value>,
    /// Where in the argument parsing stopped (a JSONPath `select`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// How to fix it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Further steps that usually fix it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

/// A failed operation: its kind, the message the caller reads, and the
/// details of an invalid argument.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// The kind every transport maps.
    pub kind: Kind,
    /// What went wrong, for a person.
    pub message: String,
    /// For an invalid argument: which one and how to fix it (boxed: the
    /// failure travels in every `Result` of the transports).
    pub details: Box<ArgumentDetails>,
}

impl Failure {
    /// A failure of `kind`.
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            details: Box::default(),
        }
    }

    /// An invalid argument.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Kind::InvalidArgument, message)
    }

    /// An internal failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Kind::Internal, message)
    }

    /// Names the argument at fault.
    #[must_use]
    pub fn parameter(mut self, name: impl Into<String>) -> Self {
        self.details.parameter = Some(name.into());
        self
    }

    /// States what the argument must be.
    #[must_use]
    pub fn expected(mut self, what: impl Into<String>) -> Self {
        self.details.expected = Some(what.into());
        self
    }

    /// Records the value that was given.
    #[must_use]
    pub fn received(mut self, value: impl Into<serde_json::Value>) -> Self {
        self.details.received = Some(value.into());
        self
    }

    /// Says how to fix it.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.details.hint = Some(hint.into());
        self
    }

    /// Adds steps that usually fix it.
    #[must_use]
    pub fn suggestions(mut self, steps: Vec<String>) -> Self {
        self.details.suggestions = steps;
        self
    }

    /// Replaces the message, keeping the kind and details (a transport's
    /// established wording for the same failure).
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// The structured details every transport may attach: the kind and the
    /// argument details.
    #[must_use]
    pub fn data(&self) -> serde_json::Value {
        #[derive(Serialize)]
        struct Data<'a> {
            kind: Kind,
            #[serde(flatten)]
            details: &'a ArgumentDetails,
        }
        // A struct of strings, an optional value and a list: always
        // representable as JSON.
        serde_json::to_value(Data {
            kind: self.kind,
            details: &self.details,
        })
        .unwrap_or_else(|_| serde_json::json!({ "kind": self.kind.as_str() }))
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// The kind of a storage port failure: a missing row is not found, a
/// refused write a conflict, a store that cannot answer unavailable, and
/// corrupt data internal.
fn port_kind(e: &PortError) -> Kind {
    match e {
        PortError::NotFound { .. } => Kind::NotFound,
        PortError::Conflict(_) => Kind::Conflict,
        PortError::Unavailable(_) => Kind::Unavailable,
        PortError::Corrupt { .. } => Kind::Internal,
    }
}

/// The kind of a failure of the shared runtime.
fn runtime_kind(e: &RuntimeError) -> Kind {
    match e {
        RuntimeError::Port(port) | RuntimeError::Persist(PersistError::Write(port)) => {
            port_kind(port)
        }
        RuntimeError::SessionNotFound(_) | RuntimeError::Persist(PersistError::SessionDeleted) => {
            Kind::NotFound
        }
        RuntimeError::Contention { .. } | RuntimeError::Persist(PersistError::HandleSuperseded) => {
            Kind::Unavailable
        }
        RuntimeError::EmbeddingsNotConfigured | RuntimeError::DimensionMismatch { .. } => {
            Kind::FailedPrecondition
        }
        RuntimeError::VectorizerGaveUp { .. } => Kind::Unavailable,
        RuntimeError::Persist(PersistError::Panicked | PersistError::WriterGone)
        | RuntimeError::InvalidConfig(_) => Kind::Internal,
    }
}

/// `kind` with the error's own text.
fn of(kind: Kind, e: &impl std::fmt::Display) -> Failure {
    Failure::new(kind, e.to_string())
}

fn record_kind(e: &RecordError) -> Kind {
    match e {
        RecordError::InvalidArgument(_) => Kind::InvalidArgument,
        RecordError::SessionNotFound(_) => Kind::NotFound,
        RecordError::Unavailable(_) => Kind::Unavailable,
        RecordError::Storage(runtime) => runtime_kind(runtime),
        RecordError::BulkItem { error, .. } => record_kind(error),
    }
}

impl From<RecordError> for Failure {
    fn from(e: RecordError) -> Self {
        of(record_kind(&e), &e)
    }
}

fn session_kind(e: &SessionError) -> Kind {
    match e {
        SessionError::NotFound(_) => Kind::NotFound,
        SessionError::InvalidArgument(_) => Kind::InvalidArgument,
        SessionError::Storage(runtime) => runtime_kind(runtime),
    }
}

impl From<SessionError> for Failure {
    fn from(e: SessionError) -> Self {
        of(session_kind(&e), &e)
    }
}

fn workspace_kind(e: &WorkspaceError) -> Kind {
    match e {
        WorkspaceError::NotFound(_) => Kind::NotFound,
        WorkspaceError::InvalidArgument(_) => Kind::InvalidArgument,
        WorkspaceError::Conflict(_) => Kind::Conflict,
        WorkspaceError::Storage(runtime) | WorkspaceError::MembershipStorage(runtime) => {
            runtime_kind(runtime)
        }
    }
}

impl From<WorkspaceError> for Failure {
    fn from(e: WorkspaceError) -> Self {
        of(workspace_kind(&e), &e)
    }
}

fn document_kind(e: &DocumentError) -> Kind {
    match e {
        DocumentError::WorkspaceNotFound(_) | DocumentError::NotFound(_) => Kind::NotFound,
        DocumentError::InvalidArgument(_) => Kind::InvalidArgument,
        DocumentError::Denied(_) => Kind::Denied,
        DocumentError::Kinds(KindsError::NotReady(_)) => Kind::FailedPrecondition,
        DocumentError::Kinds(KindsError::Unavailable(_)) => Kind::Unavailable,
        DocumentError::Kinds(KindsError::Invalid(_)) => Kind::Internal,
        DocumentError::Storage(runtime) => runtime_kind(runtime),
    }
}

impl From<DocumentError> for Failure {
    fn from(e: DocumentError) -> Self {
        of(document_kind(&e), &e)
    }
}

impl From<BlobError> for Failure {
    fn from(e: BlobError) -> Self {
        let kind = match &e {
            BlobError::WorkspaceNotFound(_) | BlobError::NotFound(_) => Kind::NotFound,
            BlobError::Refused(_) => Kind::InvalidArgument,
            BlobError::Unconfigured => Kind::FailedPrecondition,
            BlobError::Documents(documents) => document_kind(documents),
            BlobError::Storage(runtime) => runtime_kind(runtime),
        };
        of(kind, &e)
    }
}

impl From<CardError> for Failure {
    fn from(e: CardError) -> Self {
        let kind = match &e {
            CardError::WorkspaceNotFound(_) => Kind::NotFound,
            CardError::InvalidArgument(_) => Kind::InvalidArgument,
            CardError::Storage(runtime) => runtime_kind(runtime),
        };
        of(kind, &e)
    }
}

fn message_kind(e: &MessageError) -> Kind {
    match e {
        MessageError::InvalidArgument(_) => Kind::InvalidArgument,
        MessageError::SessionNotFound(_) => Kind::NotFound,
        MessageError::Storage(runtime) => runtime_kind(runtime),
    }
}

impl From<MessageError> for Failure {
    fn from(e: MessageError) -> Self {
        of(message_kind(&e), &e)
    }
}

impl From<ProjectError> for Failure {
    fn from(e: ProjectError) -> Self {
        let kind = match &e {
            ProjectError::InvalidArgument(_) => Kind::InvalidArgument,
            ProjectError::Workspace(workspace) => workspace_kind(workspace),
            ProjectError::Session(session) => session_kind(session),
            ProjectError::Storage(runtime) => runtime_kind(runtime),
        };
        of(kind, &e)
    }
}

/// The kind of a scope failure.
fn scope_kind(e: &ScopeError) -> Kind {
    match e {
        ScopeError::Missing => Kind::InvalidArgument,
        ScopeError::NotFound(_) => Kind::NotFound,
        ScopeError::Storage(runtime) => runtime_kind(runtime),
    }
}

impl From<ScopeError> for Failure {
    fn from(e: ScopeError) -> Self {
        of(scope_kind(&e), &e)
    }
}

impl From<ViewsError> for Failure {
    fn from(e: ViewsError) -> Self {
        let kind = match &e {
            ViewsError::Scope(scope) => scope_kind(scope),
            ViewsError::View(_) | ViewsError::WorkspaceNotFound(_) => Kind::NotFound,
            ViewsError::InvalidArgument(_) => Kind::InvalidArgument,
            ViewsError::RecordNotFound(_) => Kind::NotFound,
            ViewsError::Storage(runtime) => runtime_kind(runtime),
            ViewsError::Messages(messages) => message_kind(messages),
            ViewsError::Sessions(sessions) => session_kind(sessions),
            ViewsError::Search(search) => search_kind(search),
            ViewsError::RenderFailed => Kind::Internal,
        };
        of(kind, &e)
    }
}

impl From<AssemblyError> for Failure {
    fn from(e: AssemblyError) -> Self {
        let kind = match &e {
            AssemblyError::InvalidArgument(_) => Kind::InvalidArgument,
            AssemblyError::Scope(scope) => scope_kind(scope),
        };
        of(kind, &e)
    }
}

fn search_kind(e: &SearchError) -> Kind {
    match e {
        SearchError::SessionNotFound(_) | SearchError::WorkspaceNotFound(_) => Kind::NotFound,
        SearchError::EmbeddingsDisabled => Kind::FailedPrecondition,
        SearchError::InvalidArgument(_) => Kind::InvalidArgument,
        SearchError::Runtime(runtime) => runtime_kind(runtime),
    }
}

impl From<SearchError> for Failure {
    fn from(e: SearchError) -> Self {
        of(search_kind(&e), &e)
    }
}

impl From<EntityError> for Failure {
    fn from(e: EntityError) -> Self {
        let kind = match &e {
            EntityError::SessionNotFound(_) => Kind::NotFound,
            EntityError::InvalidArgument(_) => Kind::InvalidArgument,
            EntityError::Storage(runtime) => runtime_kind(runtime),
        };
        of(kind, &e)
    }
}

impl From<FreshnessError> for Failure {
    fn from(e: FreshnessError) -> Self {
        let kind = match &e {
            FreshnessError::SessionNotFound(_) => Kind::NotFound,
            FreshnessError::Storage(runtime) => runtime_kind(runtime),
        };
        of(kind, &e)
    }
}

impl From<TransferError> for Failure {
    fn from(e: TransferError) -> Self {
        let kind = match &e {
            TransferError::ModelMismatch(_) => Kind::FailedPrecondition,
            TransferError::WorkspaceNotFound(_) => Kind::NotFound,
            TransferError::Port(port) => port_kind(port),
            TransferError::SessionsNotLoaded(_) => Kind::Internal,
            TransferError::Archive(_) | TransferError::TooLarge(_) | TransferError::Empty => {
                Kind::InvalidArgument
            }
            TransferError::File(_) => Kind::Unavailable,
            TransferError::Task(_) => Kind::Internal,
        };
        of(kind, &e)
    }
}

impl From<AdminError> for Failure {
    fn from(e: AdminError) -> Self {
        let kind = match &e {
            AdminError::Runtime(runtime) => runtime_kind(runtime),
            AdminError::EmbeddingsNotStarted => Kind::FailedPrecondition,
        };
        of(kind, &e)
    }
}

impl From<UnknownVariant> for Failure {
    fn from(e: UnknownVariant) -> Self {
        Failure::invalid(e.to_string()).received(e.value)
    }
}

#[cfg(test)]
mod tests;
