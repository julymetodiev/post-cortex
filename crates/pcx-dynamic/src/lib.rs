// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `dynamic` module (docs/modules/dynamic.md): modules in Python or
//! TypeScript installed into a running host. The domain holds a bundle's rules
//! and what was approved; the service keeps, prepares, checks and asks; the
//! handlers only map. It runs nothing itself but the locked run.

#![forbid(unsafe_code)]

mod adapters;
mod directory;
mod domain;
mod handlers;
mod module;
mod ports;
mod service;
mod specs;
mod view;

pub use adapters::{LauncherConfig, host_grants};
pub use module::DynamicModule;
pub use service::memory_of;
pub use specs::{VERSION_APPROVED, VERSION_REMOVED, path};
