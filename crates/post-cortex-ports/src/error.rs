// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The one error every port returns (ARCHITECTURE sections 3 and 5):
//! [`PortError`], with the kinds a caller acts on. A record that does
//! not exist is [`PortError::NotFound`], one that cannot be read
//! [`PortError::Corrupt`] (never replaced by a default), a write that
//! lost to another [`PortError::Conflict`], and a backend that failed
//! (I/O, connection, query) [`PortError::Unavailable`].

use thiserror::Error;

/// The error of every port.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortError {
    /// The record does not exist.
    #[error("not found: {kind} {id}")]
    NotFound {
        /// Kind of record that was missing (e.g. `"session"`, `"workspace"`).
        kind: &'static str,
        /// Identifier of the missing record.
        id: String,
    },

    /// The write lost to a concurrent one; nothing was written.
    #[error("conflict: {0}")]
    Conflict(String),

    /// A stored record that cannot be read: undecodable bytes, an unknown
    /// layout marker, an unparsable timestamp or an unknown enum name.
    /// Never replaced by a default.
    #[error("corrupt {kind} {id}: {reason}")]
    Corrupt {
        /// Kind of record (e.g. `"session"`, `"update row"`).
        kind: &'static str,
        /// Identifier of the record (key or id).
        id: String,
        /// What could not be read.
        reason: String,
    },

    /// The backend failed (I/O, connection, query, encoding); the message
    /// says how.
    #[error("{0}")]
    Unavailable(String),
}

/// The result of a port method.
pub type Result<T, E = PortError> = std::result::Result<T, E>;

impl PortError {
    /// [`PortError::Unavailable`] with `error`'s message.
    pub fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }

    /// Whether the record does not exist, as opposed to a backend or
    /// decoding failure.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    /// Whether a stored record could not be read.
    pub fn is_corrupt(&self) -> bool {
        matches!(self, Self::Corrupt { .. })
    }

    /// A stable discriminant for metrics and structured logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Corrupt { .. } => "corrupt",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

/// A missing session.
pub fn session_not_found(session_id: uuid::Uuid) -> PortError {
    not_found("session", session_id)
}

/// A missing record of `kind`.
pub fn not_found(kind: &'static str, id: impl ToString) -> PortError {
    PortError::NotFound {
        kind,
        id: id.to_string(),
    }
}

/// A missing workspace.
pub fn workspace_not_found(workspace_id: uuid::Uuid) -> PortError {
    not_found("workspace", workspace_id)
}

/// A stored record that cannot be read.
pub fn corrupt(kind: &'static str, id: impl ToString, reason: impl ToString) -> PortError {
    PortError::Corrupt {
        kind,
        id: id.to_string(),
        reason: reason.to_string(),
    }
}

/// A stored RFC 3339 timestamp of record `kind` `id`; an unparsable one is
/// [`PortError::Corrupt`], never the current time.
pub fn parse_timestamp(
    kind: &'static str,
    id: impl ToString,
    value: &str,
) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| corrupt(kind, id, format!("timestamp {value:?}: {e}")))
}

/// A stored enum name of record `kind` `id`, parsed with the domain's strict
/// `FromStr` (wire name or stored name); an unknown one is
/// [`PortError::Corrupt`], never a default variant.
pub fn parse_name<T>(kind: &'static str, id: impl ToString, value: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value.parse().map_err(|e: T::Err| corrupt(kind, id, e))
}

/// A stored UUID of record `kind` `id`; an unparsable one is
/// [`PortError::Corrupt`], never skipped.
pub fn parse_uuid(kind: &'static str, id: impl ToString, value: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(value).map_err(|e| corrupt(kind, id, format!("uuid {value:?}: {e}")))
}
