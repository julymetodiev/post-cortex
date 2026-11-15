// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `http` module (docs/modules/http.md): calls of outside HTTP APIs
//! through connections that keep the organisation's credentials. The
//! domain reads the settings, plans a request (its URL under a connection
//! or to an allowed host, its credentials only to their origin), checks
//! every address it may reach and every redirect, and judges the response;
//! the service resolves, sends through the `Client` port, follows the
//! redirects and keeps the log; the handlers only map.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod service;
mod specs;

pub use module::HttpModule;
