// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A chat completions endpoint over HTTP: one streamed call per request,
//! within the scope's deadline and cancellation. A failure is classified
//! where it happens: by the error code the provider sends (its [`Codes`]),
//! else by the HTTP status, else by the transport; never by its text.

use std::time::Duration;

use pcx_module_api::Scope;
use pcx_module_api::model::{ModelFailure, ModelFailureKind, ModelRun};
use serde_json::Value;
use tokio::sync::mpsc;

use pcx_module_api::model::relay;

use crate::Answer;

/// Events buffered before the stream waits for its reader.
const QUEUE: usize = 64;

/// The longest a listing of the models may take.
const LISTING: Duration = Duration::from_secs(15);

/// The error codes a provider sends for the failures the agent acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Codes {
    /// The request is longer than the model's context.
    pub context_too_long: Vec<String>,
    /// Too many requests.
    pub rate_limited: Vec<String>,
    /// The provider is overloaded.
    pub overloaded: Vec<String>,
    /// The account's quota or plan is used up.
    pub quota_exhausted: Vec<String>,
}

impl Codes {
    /// The failure an error object (`{code, message}`) names, sent with
    /// HTTP `status` (none: within the stream).
    #[must_use]
    pub fn failure(&self, status: Option<u16>, error: &Value) -> ModelFailure {
        let code = match error.get("code") {
            Some(Value::String(code)) => Some(code.clone()),
            Some(Value::Number(code)) => Some(code.to_string()),
            _ => None,
        };
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| error.to_string(), str::to_string);
        let named = |codes: &[String]| code.as_ref().is_some_and(|c| codes.contains(c));
        let kind = if named(&self.context_too_long) {
            ModelFailureKind::ContextTooLong
        } else if named(&self.rate_limited) {
            ModelFailureKind::RateLimited
        } else if named(&self.overloaded) {
            ModelFailureKind::Overloaded
        } else if named(&self.quota_exhausted) {
            ModelFailureKind::QuotaExhausted
        } else {
            status.map_or(ModelFailureKind::Unavailable, of_status)
        };
        let message = match &code {
            Some(code) => format!("{message} (code {code})"),
            None => message,
        };
        ModelFailure::new(kind, message)
    }
}

/// The kind of a failure answered with HTTP `status`.
fn of_status(status: u16) -> ModelFailureKind {
    match status {
        401 | 403 => ModelFailureKind::Unauthorized,
        413 => ModelFailureKind::ContextTooLong,
        429 => ModelFailureKind::RateLimited,
        503 | 529 => ModelFailureKind::Overloaded,
        500..=599 => ModelFailureKind::Unavailable,
        _ => ModelFailureKind::Invalid,
    }
}

/// A transport failure: the provider could not be reached or stopped
/// answering.
fn unreachable(error: &reqwest::Error) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Unavailable, error.to_string())
}

/// A provider's chat completions URL, its key and its error codes.
#[derive(Debug, Clone)]
pub struct Endpoint {
    client: reqwest::Client,
    url: String,
    key: String,
    codes: Codes,
}

impl Endpoint {
    /// The endpoint at `url`, called with the bearer `key`.
    ///
    /// # Errors
    /// `Unavailable` when the HTTP client cannot be built (its TLS).
    pub fn new(url: &str, key: &str, codes: Codes) -> Result<Self, ModelFailure> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| unreachable(&e))?;
        Ok(Self {
            client,
            url: url.to_string(),
            key: key.to_string(),
            codes,
        })
    }

    /// The provider's listing of its models (`GET …/models`, beside its
    /// chat completions URL), its JSON.
    ///
    /// # Errors
    /// `Invalid` when the URL is no chat completions URL; the provider
    /// refused the listing (a refused key is `Unauthorized`) or could not
    /// be reached.
    pub async fn models(&self) -> Result<Value, ModelFailure> {
        let url = self
            .url
            .strip_suffix("/chat/completions")
            .map(|base| format!("{base}/models"))
            .ok_or_else(|| {
                ModelFailure::new(
                    ModelFailureKind::Invalid,
                    format!("{} is no chat completions URL", self.url),
                )
            })?;
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.key)
            .timeout(LISTING)
            .send()
            .await
            .map_err(|e| unreachable(&e))?;
        if !response.status().is_success() {
            return Err(refusal(response, &self.codes).await);
        }
        response.json().await.map_err(|e| unreachable(&e))
    }

    /// Send `body` (a streaming request) and stream its answer.
    ///
    /// # Errors
    /// The provider refused the request or could not be reached; a failure
    /// once the answer started ends its stream.
    pub async fn generate(&self, scope: &Scope, body: &Value) -> Result<ModelRun, ModelFailure> {
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.key)
            .header("Accept", "text/event-stream")
            .timeout(scope.remaining())
            .json(body)
            .send()
            .await
            .map_err(|e| unreachable(&e))?;
        if !response.status().is_success() {
            return Err(refusal(response, &self.codes).await);
        }
        let (tx, rx) = mpsc::channel(QUEUE);
        let codes = self.codes.clone();
        let cancel = scope.cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = relay(response.bytes_stream(), Answer::new(&codes), &tx) => {}
            }
        });
        Ok(ModelRun::new(rx))
    }
}

/// The failure of a request the provider refused (`response`, not a
/// success): its error object, its status and its `retry-after`.
async fn refusal(response: reqwest::Response, codes: &Codes) -> ModelFailure {
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let text = match response.text().await {
        Ok(text) => text,
        Err(e) => return unreachable(&e),
    };
    let error = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("error").cloned())
        .unwrap_or(Value::String(text));
    codes
        .failure(Some(status.as_u16()), &error)
        .with_retry_after(retry_after.as_deref())
}
