// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `workflow` module (docs/modules/workflow.md): graphs of steps that
//! run when something happens, over the tools of every module. The domain
//! holds the graph and its check (CEL through `pcx-expr`); the service
//! reaches the world through the ports; its definitions, runs and state
//! are memory's documents.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod service;
mod specs;

pub use module::WorkflowModule;

#[cfg(test)]
mod test_support;
