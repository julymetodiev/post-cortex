// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The mail module's ports filled: the system clock, and the transport
//! its settings name (an SMTP relay, Resend or Postmark).

mod http;
mod postmark;
mod resend;
mod smtp;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pcx_module_api::ModuleError;

use crate::domain::Route;
use crate::ports::{Clock, Transport};

/// The system's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The transport of `route`. Nothing connects until a send.
///
/// # Errors
/// `InvalidArgument` when an SMTP host cannot be a TLS name;
/// `Unavailable` when an HTTP client cannot be made.
pub fn transport(route: &Route) -> Result<Arc<dyn Transport>, ModuleError> {
    Ok(match route {
        Route::Smtp {
            host,
            port,
            secured,
            credentials,
        } => Arc::new(smtp::Smtp::new(host, *port, *secured, credentials.clone())?),
        Route::Resend { url, key } => Arc::new(resend::Resend::new(url, key)?),
        Route::Postmark { url, token, stream } => {
            Arc::new(postmark::Postmark::new(url, token, stream)?)
        }
    })
}

#[cfg(test)]
mod tests;
