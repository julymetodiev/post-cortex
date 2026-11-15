// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `webhooks` module (ARCHITECTURE section 8, operations, W27): a
//! source's requests (Alertmanager's first), which the daemon's
//! `POST /v1/webhooks/{source}` hands over whole, become board tasks: one
//! per incident of an alert group, commented when it fires again or
//! resolves, closed when its member found nothing to do, and the incident
//! recorded in memory. The domain holds the rules; the service applies them
//! over the board and memory (its ports); the handler only maps.

#![forbid(unsafe_code)]

mod adapters;
mod alertmanager;
mod domain;
mod handlers;
mod module;
mod ports;
mod service;

pub use module::WebhooksModule;
