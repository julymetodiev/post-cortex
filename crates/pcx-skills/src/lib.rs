// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `skills` module (ARCHITECTURE section 8): a library of skills kept
//! as documents in memory, in the workspace its configuration names, and
//! offered to every agent as MCP prompts. The domain holds the rules (a
//! skill's name, its prompt); the service applies them over its documents
//! (`pcx-documents`); the tool and prompt handlers only map.

#![forbid(unsafe_code)]

mod domain;
mod handlers;
mod module;
mod schemas;
mod service;
mod specs;
mod view;

pub use module::SkillsModule;
