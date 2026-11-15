// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Sending through a provider's HTTP API (Resend, Postmark): its request a
//! mirror message as JSON, its answer decoded into one (the proto3 JSON
//! mapping), its status read as one failure kind for both.

use std::time::Duration;

use pcx_module_api::{ModuleError, ModuleErrorKind};
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// How long a send may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What a provider's status says of a send that failed: refused
/// credentials, a refused message, a limited rate, or a failure that may
/// pass on a later try.
#[must_use]
pub fn failure(status: StatusCode) -> ModuleErrorKind {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ModuleErrorKind::Denied,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            ModuleErrorKind::InvalidArgument
        }
        StatusCode::TOO_MANY_REQUESTS => ModuleErrorKind::ResourceExhausted,
        _ => ModuleErrorKind::Unavailable,
    }
}

/// A provider's API.
pub struct Api {
    http: reqwest::Client,
    /// Its name, which its failures say.
    name: &'static str,
}

impl Api {
    /// The API of provider `name`.
    ///
    /// # Errors
    /// `Unavailable` when the HTTP client cannot be made.
    pub fn new(name: &'static str) -> Result<Self, ModuleError> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| ModuleError::new(ModuleErrorKind::Unavailable, format!("{name}: {e}")))?;
        Ok(Self { http, name })
    }

    /// A failure of `kind` that says `what`.
    pub fn failed(&self, kind: ModuleErrorKind, what: impl std::fmt::Display) -> ModuleError {
        ModuleError::new(kind, format!("{}: {what}", self.name))
    }

    /// POST `body` to `url` with `headers`: the answer's status and its
    /// body decoded.
    ///
    /// # Errors
    /// `Unavailable` when it cannot be sent, or its answer does not come or
    /// does not decode (a proxy's page instead of the provider's JSON).
    pub async fn post<I: Serialize, O: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &I,
    ) -> Result<(StatusCode, O), ModuleError> {
        let mut request = self
            .http
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        // The headers hold the credentials: a failure says only what failed.
        let response = request.send().await.map_err(|e| {
            self.failed(
                ModuleErrorKind::Unavailable,
                format!("could not be reached ({})", reached(&e)),
            )
        })?;
        let status = response.status();
        let answer = response.bytes().await.map_err(|e| {
            self.failed(
                ModuleErrorKind::Unavailable,
                format!("its answer did not come ({})", reached(&e)),
            )
        })?;
        let decoded = serde_json::from_slice(&answer).map_err(|e| {
            self.failed(
                failure(status),
                format!("its answer (HTTP {status}) is not its JSON: {e}"),
            )
        })?;
        Ok((status, decoded))
    }
}

/// What went wrong with a request, without its URL or headers.
fn reached(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "no connection"
    } else {
        "the request failed"
    }
}
