// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Transports of pcx (ARCHITECTURE section 3, `post-cortex-transport`).
//!
//! - [`mcp`]: the MCP server (rmcp) over the host's tools;
//! - [`invoke`]: the same tools over gRPC (`pcx.module.v1.Host`);
//! - [`views`]: the app's views over gRPC (`pcx.views.v1.Views`);
//! - [`transfer`]: archive export and import (`pcx.v1` `Export`, `Import`),
//!   for gRPC and the browser;
//! - [`browser`]: what a browser needs: the origin allowlist (CORS) and
//!   archive transfer over plain HTTP;
//! - [`grpc`]: the gRPC status of a failed memory operation;
//! - [`classify`]: a request of any transport as the operation table sees
//!   it, for a gateway in front of many memory instances.
//!
//! Transports contain no logic: a handler maps the wire format onto a host
//! or capability call and the result back.

#![forbid(unsafe_code)]

pub mod agent;
pub mod blobs;
pub mod blocks;
pub mod browser;
pub mod classify;
pub mod grpc;
pub mod host_api;
pub mod job_health;
pub mod mcp;
pub mod memory;
pub mod modules;
pub mod person;
pub mod reach;
pub mod rest;
pub mod transfer;
pub mod views;
pub mod webhooks;

#[cfg(test)]
mod table_tests;
