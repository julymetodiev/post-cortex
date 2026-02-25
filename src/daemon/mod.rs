// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! HTTP daemon server for multi-client access to Post-Cortex
//!
//! Provides lock-free HTTP/SSE transport for MCP JSON-RPC, enabling multiple
//! Claude Code instances to share a single RocksDB instance.

pub mod config;
pub mod coerce;
pub mod format_helpers;
pub mod mcp_service;
pub mod rmcp_server;
pub mod server;
pub mod sse;
pub mod stdio_proxy;
pub mod validate;

pub use config::DaemonConfig;
pub use mcp_service::PostCortexService;
pub use rmcp_server::start_rmcp_daemon;
pub use server::DaemonServer;
pub use sse::SSEBroadcaster;
pub use stdio_proxy::{ensure_daemon_running, is_daemon_running, run_stdio_proxy};
