// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Anthropic's API over HTTP: its listing of models, page by page, and one
//! streamed Messages call per request, within the scope's deadline and
//! cancellation. A failure is classified where it happens: by the error's
//! type, else by the HTTP status, else by the transport; never by its
//! text.

use std::time::Duration;

use pcx_module_api::Scope;
use pcx_module_api::model::{ModelFailure, ModelFailureKind, ModelRun, relay};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::answer::Answer;
use crate::catalog::VERSION;

/// Events buffered before the stream waits for its reader.
const QUEUE: usize = 64;

/// The longest a page of the listing may take.
const LISTING: Duration = Duration::from_secs(15);

/// The most pages of the listing read (a thousand models each).
const PAGES: usize = 10;

/// The failure an error object (`{type, message}`) names, sent with HTTP
/// `status` (none: within the stream).
pub fn failure(status: Option<u16>, error: &Value) -> ModelFailure {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| error.to_string(), str::to_string);
    let kind = match error.get("type").and_then(Value::as_str) {
        Some("rate_limit_error") => ModelFailureKind::RateLimited,
        Some("overloaded_error") => ModelFailureKind::Overloaded,
        Some("authentication_error" | "permission_error") => ModelFailureKind::Unauthorized,
        Some("billing_error") => ModelFailureKind::QuotaExhausted,
        Some("request_too_large") => ModelFailureKind::ContextTooLong,
        Some("api_error") => ModelFailureKind::Unavailable,
        Some("invalid_request_error" | "not_found_error") => ModelFailureKind::Invalid,
        _ => status.map_or(ModelFailureKind::Unavailable, of_status),
    };
    ModelFailure::new(kind, message)
}

/// The kind of a failure answered with HTTP `status`.
fn of_status(status: u16) -> ModelFailureKind {
    match status {
        401 | 403 => ModelFailureKind::Unauthorized,
        402 => ModelFailureKind::QuotaExhausted,
        413 => ModelFailureKind::ContextTooLong,
        429 => ModelFailureKind::RateLimited,
        503 | 529 => ModelFailureKind::Overloaded,
        500..=599 => ModelFailureKind::Unavailable,
        _ => ModelFailureKind::Invalid,
    }
}

/// A transport failure: the API could not be reached or stopped answering.
fn unreachable(error: &reqwest::Error) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Unavailable, error.to_string())
}

/// The failure of a request the API refused (`response`, not a success):
/// its error object, its status and its `retry-after`.
async fn refusal(response: reqwest::Response) -> ModelFailure {
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
    failure(Some(status.as_u16()), &error).with_retry_after(retry_after.as_deref())
}

/// Anthropic's API at its URL, called with an API key.
#[derive(Debug, Clone)]
pub struct Endpoint {
    client: reqwest::Client,
    base: String,
    key: String,
}

impl Endpoint {
    /// The API at `base` (`https://api.anthropic.com`), called with `key`.
    ///
    /// # Errors
    /// `Unavailable` when the HTTP client cannot be built (its TLS).
    pub fn new(base: &str, key: &str) -> Result<Self, ModelFailure> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| unreachable(&e))?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
    }

    /// The entries of its listing (`GET /v1/models`), every page.
    ///
    /// # Errors
    /// The API refused the listing (a refused key is `Unauthorized`) or
    /// could not be reached.
    pub async fn models(&self) -> Result<Vec<Value>, ModelFailure> {
        let mut entries = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..PAGES {
            let mut query = vec![("limit", "1000".to_string())];
            if let Some(id) = after.take() {
                query.push(("after_id", id));
            }
            let response = self
                .request(reqwest::Method::GET, "/v1/models")
                .query(&query)
                .timeout(LISTING)
                .send()
                .await
                .map_err(|e| unreachable(&e))?;
            if !response.status().is_success() {
                return Err(refusal(response).await);
            }
            let page: Value = response.json().await.map_err(|e| unreachable(&e))?;
            if let Some(data) = page.get("data").and_then(Value::as_array) {
                entries.extend(data.iter().cloned());
            }
            let more = page.get("has_more").and_then(Value::as_bool) == Some(true);
            after = page
                .get("last_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            if !more || after.is_none() {
                break;
            }
        }
        Ok(entries)
    }

    /// Send `body` (a streaming request) and stream its answer.
    ///
    /// # Errors
    /// The API refused the request or could not be reached; a failure once
    /// the answer started ends its stream.
    pub async fn generate(&self, scope: &Scope, body: &Value) -> Result<ModelRun, ModelFailure> {
        let response = self
            .request(reqwest::Method::POST, "/v1/messages")
            .header("accept", "text/event-stream")
            .timeout(scope.remaining())
            .json(body)
            .send()
            .await
            .map_err(|e| unreachable(&e))?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        let (tx, rx) = mpsc::channel(QUEUE);
        let cancel = scope.cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = relay(response.bytes_stream(), Answer::new(), &tx) => {}
            }
        });
        Ok(ModelRun::new(rx))
    }
}

#[cfg(test)]
mod tests;
