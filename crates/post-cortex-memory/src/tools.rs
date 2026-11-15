// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The memory tools (ARCHITECTURE section 7) and what every transport of
//! memory shares:
//!
//! - [`operations`]: the operation table, the single list of user-facing
//!   operations, their capability method and the MCP tool / action, gRPC RPC
//!   and REST route that expose them;
//! - [`responses`]: one serde response shape per operation, used by every
//!   transport and by [`select`];
//! - [`Failure`]: the one classification of capability errors that every
//!   transport maps in one place;
//! - [`wire`]: the shared strict parsers (enum names, entities, relations,
//!   validity, sources, modes);
//! - the nine tools of [`crate::MemoryModule`]: their argument types and
//!   schemas, handlers and replies.
//!
//! A tool handler parses its arguments into the typed request, calls the
//! capability method and maps the result back.

mod announce;
mod args;
mod catalog;
mod check;
mod code_ref;
#[cfg(test)]
mod docs;
mod events;
mod failure;
mod handlers;
mod module;
pub mod operations;
mod parse;
mod reply;
pub mod responses;
pub mod select;
pub mod wire;

pub use catalog::SchemaError;
pub use events::CapturedMessage;
pub use failure::{ArgumentDetails, Failure, Kind};
pub use module::MemoryModule;
