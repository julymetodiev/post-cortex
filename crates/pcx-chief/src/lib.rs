// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `chief` module (ARCHITECTURE section 8): the host's main agent. A
//! question (`chief_ask`) runs a turn of the agent in a conversation of
//! its own, under the grant the host enforces and a daily budget; every
//! tool call of the turn is recorded. The domain holds the rules; the
//! service applies them over its documents and sessions (`pcx-documents`),
//! the agent's turns and a clock; the tool handler only maps.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod schemas;
mod service;
mod settings;
mod specs;
mod turns;
mod view;
mod wiring;

pub use module::ChiefModule;
