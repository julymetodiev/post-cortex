// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Typed error hierarchy for `post-cortex-daemon`.
//!
//! Carries variants for the transport surface (HTTP / gRPC /
//! stdio) and delegates domain errors downward. [`Error::grpc_status`]
//! maps each variant to a `tonic::Status`; [`Error::http_status`] gives
//! the matching `http::StatusCode` for axum handlers.

use thiserror::Error;

/// Errors produced by the daemon's transport layer.
#[derive(Debug, Error)]
pub enum Error {
    /// Caller request failed validation (invalid UUID, missing field).
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Storage error surfaced to the transport.
    #[error(transparent)]
    Storage(#[from] post_cortex_ports::PortError),

    /// I/O error (stdio bridge, TCP, file).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Catch-all for migrating call sites that still use `anyhow`.
    #[error(transparent)]
    External(#[from] anyhow::Error),
}

/// Crate-level result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// A stable discriminant for metrics / structured logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::Storage(_) => "storage",
            Self::Io(_) => "io",
            Self::External(_) => "external",
        }
    }

    /// Map this error to a `tonic::Status` for the gRPC surface.
    #[must_use]
    pub fn grpc_status(&self) -> tonic::Status {
        match self {
            Self::InvalidRequest(msg) => tonic::Status::invalid_argument(msg),
            _ => tonic::Status::internal(self.to_string()),
        }
    }

    /// Map this error to an HTTP status code for the axum surface.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        match self {
            Self::InvalidRequest(_) => 400,
            _ => 500,
        }
    }
}
