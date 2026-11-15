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

//! The Post-Cortex daemon: one process that owns the storage and serves MCP
//! (streamable HTTP and stdio), gRPC and REST to every client.

pub mod auth;
pub mod bootstrap;
pub mod config;
mod connect;
#[cfg(test)]
mod connect_runner_tests;
#[cfg(test)]
mod connect_tests;
mod grpc_server;
#[cfg(test)]
mod grpc_tests;
pub mod host_allowlist;
#[cfg(test)]
mod mcp_prompts_tests;
#[cfg(test)]
mod mcp_tests;
pub mod observability;
pub mod pidfile;
mod proxies;
mod reload;
pub mod rmcp_server;
#[cfg(test)]
mod router_tests;
mod settings_file;
mod settings_grants;
#[cfg(test)]
mod test_client;

pub use bootstrap::start_rmcp_daemon;
pub use config::{ConfigError, DaemonConfig};
pub use grpc_server::start_grpc_server;
pub use observability::{
    TracingInitError, init as init_observability, shutdown as shutdown_observability,
};
