// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! HTTP / gRPC / SSE / stdio daemon for post-cortex.
//!
//! Hosts:
//!
//! - the **rmcp Model Context Protocol** surface (streaming-HTTP +
//!   stdio transports) via [`daemon::rmcp_server`] and
//!   [`daemon::mcp_service`];
//! - the **tonic gRPC API** (canonical post-cortex wire surface) via
//!   [`daemon::grpc_service`];
//! - the **axum HTTP server** with REST endpoints for the `pcx` CLI
//!   client and the Server-Sent Events broadcast plumbing via
//!   [`daemon::server`] and [`daemon::sse`];
//! - the **stdio bridge** that lets MCP hosts spawn the daemon as a
//!   child process via [`daemon::stdio_proxy`];
//! - the unified **`pcx` CLI binary** (`[[bin]]`) under `src/bin/pcx`.
//!
//! The daemon owns no domain logic — every read / write / search /
//! manage operation delegates downward into
//! [`post_cortex_memory::ConversationMemorySystem`] (and, from Phase 7
//! onwards, through the
//! [`post_cortex_core::services::PostCortexService`] trait). Validation
//! that was historically duplicated between the gRPC and MCP layers
//! migrates to the trait impl in `post-cortex-memory::MemoryServiceImpl`.

// rustdoc::broken_intra_doc_links is warned (not denied) for the daemon
// because the migrated grpc_service / mcp_service files carry historical
// `[private_item]` references; cleanup is Phase 12 follow-up after the
// transport handlers migrate to call PostCortexService.
#![warn(rustdoc::broken_intra_doc_links)]
// The daemon's only unsafe is in two test cases that exercise env var
// overrides for DaemonConfig (`set_var` / `remove_var` are `unsafe fn`
// since Rust 1.85 because env mutation is racy in multi-threaded
// programs). Both blocks are gated under #[cfg(test)] and serialised by
// the test framework, so we relax `forbid` to `deny` and let those two
// blocks opt in via `#[allow(unsafe_code)]`.
#![deny(unsafe_code)]

pub mod daemon;
pub mod error;

pub use daemon::config::DaemonConfig;
pub use error::{Error, Result};
