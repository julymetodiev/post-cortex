// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The gRPC side of the failure mapping: every capability error becomes a
//! [`Status`] through its [`Failure`] kind (ARCHITECTURE rule 5), for the
//! daemon's `pcx.v1` service and every other gRPC adapter over the memory
//! capabilities; and a failed call's [`Status`] as the HTTP answer of a
//! route that is not gRPC (the browser transfer, the webhooks).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tonic::{Code, Status};

use post_cortex_memory::tools::{Failure, Kind};

/// The one mapping of a failed operation onto a gRPC status.
pub fn status(e: impl Into<Failure>) -> Status {
    let failure = e.into();
    let message = match &failure.details.hint {
        Some(hint) => format!("{}. {hint}", failure.message),
        None => failure.message,
    };
    match failure.kind {
        Kind::InvalidArgument => Status::invalid_argument(message),
        Kind::NotFound => Status::not_found(message),
        Kind::Conflict => Status::already_exists(message),
        Kind::Denied => Status::permission_denied(message),
        Kind::FailedPrecondition => Status::failed_precondition(message),
        Kind::Unavailable => Status::unavailable(message),
        Kind::Internal => Status::internal(message),
    }
}

/// The HTTP answer of a failed call: its status by code, its message.
pub fn http_failure(status: &Status) -> Response {
    let code = match status.code() {
        Code::InvalidArgument | Code::OutOfRange => StatusCode::BAD_REQUEST,
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::AlreadyExists | Code::Aborted => StatusCode::CONFLICT,
        Code::FailedPrecondition => StatusCode::PRECONDITION_FAILED,
        Code::Unauthenticated => StatusCode::UNAUTHORIZED,
        Code::PermissionDenied => StatusCode::FORBIDDEN,
        Code::ResourceExhausted | Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, format!("{}\n", status.message())).into_response()
}
