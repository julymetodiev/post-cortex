// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the http module needs of the world (docs/modules/http.md, layers)
//! besides its documents (`pcx-documents`): the time, a host's addresses,
//! and a client that sends one checked request to checked addresses. The
//! module's adapters fill them (the system's resolver, reqwest); tests fill
//! them in memory.

use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::ModuleError;

use crate::domain::request::Planned;

/// The time.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> DateTime<Utc>;
}

/// A host's addresses.
#[async_trait]
pub trait Resolver: Send + Sync {
    /// The addresses of `host` at `port`.
    ///
    /// # Errors
    /// `Unavailable` when it does not resolve.
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, ModuleError>;
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// The status.
    pub status: u16,
    /// Its reason, as the status names it.
    pub reason: String,
    /// The headers, as they came.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
    /// How long it took.
    pub took: Duration,
}

/// Sends a request.
#[async_trait]
pub trait Client: Send + Sync {
    /// Send `request` to one of `addresses` (its host's, checked), its body
    /// read up to `limit` bytes; follow no redirect.
    ///
    /// # Errors
    /// `DeadlineExceeded` past its time limit; `ResourceExhausted` for a
    /// body above `limit`; `Unavailable` when it cannot be sent or no
    /// answer comes.
    async fn send(
        &self,
        request: &Planned,
        addresses: &[SocketAddr],
        limit: usize,
    ) -> Result<Received, ModuleError>;
}
