// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a browser needs to use a memory instance (the open app on the
//! web): the origins allowed to call it ([`Origins`], answered by [`cors`])
//! and archive transfer over plain HTTP ([`transfer`]), since gRPC-Web
//! cannot stream a request from a browser. gRPC-Web itself is a layer over
//! the gRPC routes (the composition adds it).
//!
//! Browsers authenticate as every client does, with the bearer token in
//! `Authorization`; no cookie is involved, so CORS answers without
//! credentials. A daemon allows no origin unless configured.

use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};

mod tickets;
pub mod transfer;

/// How long a browser may cache a preflight answer.
const PREFLIGHT_MAX_AGE: Duration = Duration::from_secs(3600);

/// An origin entry that is not `localhost` nor `http(s)://host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("browser origin {0:?} is not http(s)://host[:port] or localhost")]
pub struct InvalidOrigin(pub String);

/// The origins allowed to call: exact `scheme://host[:port]` entries, and
/// `localhost` for any `http://localhost[:port]` or `http://127.0.0.1[:port]`
/// (development).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Origins {
    exact: Vec<HeaderValue>,
    localhost: bool,
}

impl Origins {
    /// The configured entries (a trailing `/` is ignored).
    ///
    /// # Errors
    /// An entry is neither `localhost` nor `http(s)://host[:port]`.
    pub fn parse(entries: &[String]) -> Result<Self, InvalidOrigin> {
        let mut origins = Self::default();
        for raw in entries {
            let entry = raw.trim().trim_end_matches('/');
            if entry == "localhost" {
                origins.localhost = true;
                continue;
            }
            let host = entry
                .strip_prefix("https://")
                .or_else(|| entry.strip_prefix("http://"));
            let valid = host.is_some_and(|host| {
                !host.is_empty()
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
            });
            if !valid {
                return Err(InvalidOrigin(raw.clone()));
            }
            let value = HeaderValue::from_str(entry).map_err(|_| InvalidOrigin(raw.clone()))?;
            origins.exact.push(value);
        }
        Ok(origins)
    }

    /// Whether no origin is allowed.
    pub fn is_empty(&self) -> bool {
        self.exact.is_empty() && !self.localhost
    }

    /// Whether `origin` may call.
    pub fn allows(&self, origin: &HeaderValue) -> bool {
        if self.exact.contains(origin) {
            return true;
        }
        let o = origin.as_bytes();
        self.localhost
            && ["http://localhost", "http://127.0.0.1"].iter().any(|p| {
                o.strip_prefix(p.as_bytes())
                    .is_some_and(|rest| rest.is_empty() || is_port(rest))
            })
    }
}

fn is_port(rest: &[u8]) -> bool {
    rest.strip_prefix(b":")
        .is_some_and(|p| !p.is_empty() && p.iter().all(u8::is_ascii_digit))
}

/// The CORS answer for `origins`: their requests (gRPC-Web, REST, MCP and
/// transfer) with the bearer token, and the gRPC status and download name
/// readable by the page.
pub fn cors(origins: Origins) -> CorsLayer {
    let name = HeaderName::from_static;
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, _| {
            origins.allows(origin)
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            name("x-grpc-web"),
            name("x-user-agent"),
            name("grpc-timeout"),
            name("mcp-session-id"),
            name("mcp-protocol-version"),
        ])
        .expose_headers([
            name("grpc-status"),
            name("grpc-message"),
            name("grpc-status-details-bin"),
            name("mcp-session-id"),
            header::CONTENT_DISPOSITION,
        ])
        .max_age(PREFLIGHT_MAX_AGE)
}

#[cfg(test)]
mod tests;
