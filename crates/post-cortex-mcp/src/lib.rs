// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Model Context Protocol (MCP) tool definitions for post-cortex.
//!
//! Pure library — no `rmcp`, `axum`, `tonic`, or transport dependencies.
//! Each tool function takes a `&dyn post_cortex_core::PostCortexService`
//! and returns an `MCPToolResult`, so this crate can be embedded in any
//! MCP server runtime (the post-cortex daemon is one of many possible
//! hosts).
//!
//! Status: **Phase 1 stub.** Tool functions migrate from
//! `src/tools/mcp/` in Phase 6.

#![forbid(unsafe_code)]
#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
