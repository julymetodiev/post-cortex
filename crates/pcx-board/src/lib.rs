// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `board` module (ARCHITECTURE section 8): boards, tasks, their
//! discussion and decisions, kept as documents in memory. The domain holds
//! the rules; the service applies them over its ports (documents and
//! records in memory, a branch's commits in `tools-git`), filled by
//! adapters over the host handle; the tool handlers only map JSON.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod schemas;
mod service;
mod specs;
mod team;
mod view;

pub use module::BoardModule;
