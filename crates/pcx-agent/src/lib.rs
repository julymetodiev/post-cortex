// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The pcx agent (ARCHITECTURE section 7, the agent): a turn with models
//! and tools until the model answers, every turn of it kept in the
//! session's transcript in memory, every step heard by the modules that
//! answer its events. The domain holds the rules (the conversation, what
//! follows an answer or a failure); the service runs a turn over its
//! ports, filled by adapters over the host handle.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod module;
mod ports;
mod service;

pub use module::AgentModule;
