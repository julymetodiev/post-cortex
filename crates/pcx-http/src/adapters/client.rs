// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The client over reqwest: one request to the addresses the service
//! checked (its host resolved to them only, no proxy, no redirect of its
//! own), its body read up to the limit. A failure says only what failed,
//! never the URL or the headers, which hold the credentials.

use std::net::SocketAddr;
use std::time::Instant;

use async_trait::async_trait;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use reqwest::redirect::Policy;

use crate::domain::request::{Method, Planned};
use crate::ports::{Client, Received};

/// What the client calls itself, unless a connection or a request says
/// otherwise (GitHub refuses a request without one).
const USER_AGENT: &str = concat!("pcx-http/", env!("CARGO_PKG_VERSION"));

/// The client over reqwest.
pub struct Reqwest;

/// What went wrong with a request, without its URL or headers.
fn failed(e: &reqwest::Error, what: &str) -> ModuleError {
    let (kind, why) = if e.is_timeout() {
        (ModuleErrorKind::DeadlineExceeded, "timed out")
    } else if e.is_connect() {
        (ModuleErrorKind::Unavailable, "no connection")
    } else {
        (ModuleErrorKind::Unavailable, "the request failed")
    };
    ModuleError::new(kind, format!("{what}: {why}"))
}

fn method(m: Method) -> reqwest::Method {
    match m {
        Method::Get => reqwest::Method::GET,
        Method::Head => reqwest::Method::HEAD,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
    }
}

#[async_trait]
impl Client for Reqwest {
    async fn send(
        &self,
        request: &Planned,
        addresses: &[SocketAddr],
        limit: usize,
    ) -> Result<Received, ModuleError> {
        let host = request.url.host_str().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "the request's URL has no host",
            )
        })?;
        let what = format!("{} {host}", request.method.as_str());
        // A client per request: its host reaches only the addresses checked.
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .user_agent(USER_AGENT)
            .timeout(request.timeout)
            .resolve_to_addrs(host, addresses)
            .build()
            .map_err(|e| failed(&e, &what))?;
        let mut call = client.request(method(request.method), request.url.clone());
        for (name, value) in &request.headers {
            call = call.header(name.as_str(), value.as_str());
        }
        if let Some(body) = &request.body {
            call = call
                .header(reqwest::header::CONTENT_TYPE, body.content_type.as_str())
                .body(body.bytes.clone());
        }
        let started = Instant::now();
        let mut response = call.send().await.map_err(|e| failed(&e, &what))?;
        let too_big = || {
            ModuleError::new(
                ModuleErrorKind::ResourceExhausted,
                format!("{what}: the response's body is above {limit} bytes"),
            )
        };
        if response
            .content_length()
            .is_some_and(|n| usize::try_from(n).map_or(true, |n| n > limit))
        {
            return Err(too_big());
        }
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
                (name.as_str().to_string(), value)
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| failed(&e, &what))? {
            if body.len() + chunk.len() > limit {
                return Err(too_big());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Received {
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or_default().to_string(),
            headers,
            body,
            took: started.elapsed(),
        })
    }
}
