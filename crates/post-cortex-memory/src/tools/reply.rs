// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! What a tool answers: its text and, when the operation has data, its
//! response as JSON (ARCHITECTURE rule 5). Transports map a
//! [`ToolOutput`] and a failure's [`ModuleError`] onto their wire in one
//! place each.

use pcx_module_api::{ModuleError, ModuleErrorKind, ToolOutput};
use serde::Serialize;

use crate::tools::select::Select;
use crate::tools::{Failure, Kind};

/// A reply with `response` as its data.
///
/// # Errors
/// `response` cannot be represented as JSON.
pub fn with<T: Serialize>(text: impl Into<String>, response: &T) -> Result<ToolOutput, Failure> {
    let data = serde_json::to_value(response)
        .map_err(|e| Failure::internal(format!("response not serialisable: {e}")))?;
    Ok(ToolOutput {
        text: text.into(),
        data: Some(data),
        failed: false,
    })
}

/// A failed call that still answers with `response` (a bulk write's
/// per-item results).
///
/// # Errors
/// `response` cannot be represented as JSON.
pub fn failed<T: Serialize>(text: impl Into<String>, response: &T) -> Result<ToolOutput, Failure> {
    let mut reply = with(text, response)?;
    reply.failed = true;
    Ok(reply)
}

/// A reply with the part of `response` that `select` picks (all of it
/// without a select).
///
/// # Errors
/// The select is invalid, or the response is not serialisable.
pub fn selected<T: Serialize>(
    text: impl Into<String>,
    response: &T,
    select: &Select,
) -> Result<ToolOutput, Failure> {
    if select.is_empty() {
        return with(text, response);
    }
    with(text, &select.apply(response)?)
}

impl From<Failure> for ModuleError {
    /// The failure's kind, message, and its details as data.
    fn from(failure: Failure) -> Self {
        let kind = match failure.kind {
            Kind::InvalidArgument => ModuleErrorKind::InvalidArgument,
            Kind::NotFound => ModuleErrorKind::NotFound,
            Kind::Conflict => ModuleErrorKind::Conflict,
            Kind::Denied => ModuleErrorKind::Denied,
            Kind::FailedPrecondition => ModuleErrorKind::FailedPrecondition,
            Kind::Unavailable => ModuleErrorKind::Unavailable,
            Kind::Internal => ModuleErrorKind::Internal,
        };
        let details = failure.data();
        ModuleError::new(kind, failure.message).with_details(details)
    }
}
