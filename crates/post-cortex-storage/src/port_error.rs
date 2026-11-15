// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Where an adapter's internal errors become the ports' typed
//! [`PortError`]: one error type for the bodies of both backends, and one
//! conversion for every port method.

use post_cortex_ports::PortError;
use uuid::Uuid;

/// What an adapter body fails with: the backends' own errors, the
/// conditions the adapters check, and the [`PortError`]s a body raises
/// itself (not found, corrupt, ...).
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// Raised by the body; keeps its kind.
    #[error(transparent)]
    Port(#[from] PortError),
    /// RocksDB failed.
    #[error("rocksdb: {0}")]
    RocksDb(#[from] rocksdb::Error),
    /// SurrealDB failed.
    #[cfg(feature = "surrealdb-storage")]
    #[error("surrealdb: {0}")]
    SurrealDb(#[from] surrealdb::Error),
    /// A value could not be encoded as JSON.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A value could not be encoded as a row.
    #[error("row encoding: {0}")]
    Encode(#[from] bincode::error::EncodeError),
    /// The file system failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A blocking database task panicked or was cancelled.
    #[error("blocking task: {0}")]
    Join(#[from] tokio::task::JoinError),
    /// A count or position is out of the database's integer range.
    #[error("integer out of range: {0}")]
    Range(#[from] std::num::TryFromIntError),
    /// A save named another session than the snapshot's.
    #[error("session id mismatch: snapshot {snapshot}, updates for {updates}")]
    SessionMismatch {
        /// The snapshot's session.
        snapshot: Uuid,
        /// The session the updates were given for.
        updates: Uuid,
    },
    /// An embedding of another length than the configured one.
    #[error("embedding dimension: expected {expected}, got {got}")]
    Dimension {
        /// The configured dimension.
        expected: usize,
        /// The embedding's length.
        got: usize,
    },
    /// An embedding shorter than any model's (no dimension configured).
    #[error("embedding too short: {got} dims, need at least {min}")]
    TooShort {
        /// The embedding's length.
        got: usize,
        /// The shortest accepted.
        min: usize,
    },
    /// A query that always returns a row returned none.
    #[error("no {0} row returned")]
    NoRow(&'static str),
    /// Statements of a session save failed; the save did not land whole.
    #[error("saving session {session_id}: {failed} statement(s) failed")]
    SaveFailed {
        /// The session.
        session_id: Uuid,
        /// How many statements failed.
        failed: usize,
    },
}

impl From<AdapterError> for PortError {
    fn from(e: AdapterError) -> Self {
        match e {
            AdapterError::Port(typed) => typed,
            other => PortError::Unavailable(other.to_string()),
        }
    }
}

/// The result of an adapter body.
pub type Result<T, E = AdapterError> = std::result::Result<T, E>;

/// The port result of an adapter body.
pub fn port<T>(result: Result<T>) -> PortResult<T> {
    result.map_err(PortError::from)
}

/// The port result type.
pub type PortResult<T> = post_cortex_ports::error::Result<T>;
