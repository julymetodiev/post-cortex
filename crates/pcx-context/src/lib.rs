// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The pcx context engine (ARCHITECTURE section 7, the agent): the module
//! in the `context_engine` slot. It gives a turn what memory recalls for
//! its prompt (memory's `AssembleContext`) and sums up a conversation too
//! long for the model, through the model, keeping the summary as a record
//! of the session. The domain holds the summary's request; the service
//! runs over its ports, filled by adapters over the host handle.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod module;
mod ports;
mod service;

pub use module::ContextModule;
