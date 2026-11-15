// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! HTTP / gRPC / stdio daemon for post-cortex.
//!
//! Hosts:
//!
//! - the **rmcp Model Context Protocol** surface (streaming-HTTP +
//!   stdio transports) via [`daemon::rmcp_server`], serving
//!   `post_cortex_transport::mcp::McpService` over the module host;
//! - the **tonic gRPC API** (canonical post-cortex wire surface) via
//!   `post_cortex_transport::memory`, served by [`daemon::start_grpc_server`];
//! - the **REST endpoints** (`/api/*`, `/health`) for the `pcx` CLI and
//!   hooks (`post_cortex_transport::rest`), served by the same axum app as
//!   MCP;
//! - [`compose::routes`]: every route of a memory instance (MCP, REST and,
//!   for browsers, gRPC-Web and archive transfer), served by the daemon and
//!   by every hosted tenant;
//! - the unified **`pcx` binary** (`[[bin]]`) under `src/bin/pcx`: the
//!   daemon's own commands (start, stop, the admin commands, through the
//!   running daemon or offline) and the client commands of
//!   `post_cortex_cli` (setup, profiles, hooks, the stdio MCP bridge).
//!
//! The daemon owns no domain logic: every operation is a method of the
//! capability services ([`post_cortex_memory::Memory`]) or a tool of the
//! module host (`pcx_host::Host` with `post_cortex_memory::MemoryModule`);
//! the operation table, response shapes and error classification are in
//! `post_cortex_memory::tools`.

// The daemon's only unsafe is in two test cases that exercise env var
// overrides for DaemonConfig (`set_var` / `remove_var` are `unsafe fn`
// since Rust 1.85 because env mutation is racy in multi-threaded
// programs). Both blocks are gated under #[cfg(test)] and serialised by
// the test framework, so we relax `forbid` to `deny` and let those two
// test blocks opt in to unsafe code.
#![deny(unsafe_code)]

pub mod compose;
pub mod daemon;
/// Error types for the daemon crate.
pub mod error;

#[cfg(test)]
mod compose_agent_tests;
#[cfg(test)]
mod compose_board_tests;
#[cfg(test)]
mod compose_chief_tests;
#[cfg(test)]
mod compose_documents_tests;
#[cfg(all(test, target_os = "macos"))]
mod compose_dynamic_agent_tests;
#[cfg(all(test, target_os = "macos"))]
mod compose_dynamic_stack_tests;
#[cfg(all(test, target_os = "macos"))]
mod compose_dynamic_tests;
#[cfg(test)]
mod compose_events_tests;
#[cfg(test)]
mod compose_live_ticket_tests;
#[cfg(test)]
mod compose_mcp_silence_tests;
#[cfg(test)]
mod compose_modules_tests;
#[cfg(test)]
mod compose_ops_tests;
#[cfg(test)]
mod compose_reach_tests;
#[cfg(test)]
mod compose_resume_tests;
#[cfg(test)]
mod compose_routes_tests;
#[cfg(test)]
mod compose_runner_tests;
#[cfg(test)]
mod compose_scheduler_tests;
#[cfg(test)]
mod compose_scripted_tests;
#[cfg(test)]
mod compose_skills_tests;
#[cfg(test)]
mod compose_stop_turn_tests;
#[cfg(test)]
mod compose_store_tests;
#[cfg(test)]
mod compose_team_tests;
#[cfg(test)]
mod compose_telegram_tests;
#[cfg(test)]
mod compose_tests;
#[cfg(test)]
mod compose_webhooks_tests;
#[cfg(test)]
mod compose_workflow_tests;
#[cfg(test)]
mod memory_tests;

pub use daemon::config::DaemonConfig;
pub use error::{Error, Result};
