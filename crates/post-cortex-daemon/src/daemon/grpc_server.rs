// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Serving the daemon's gRPC routes.

use std::sync::Arc;

use post_cortex_memory::Memory;
use tracing::info;

/// Serve gRPC over `memory` (the daemon's capability services, shared with
/// MCP and REST) on `addr` until `shutdown` is cancelled; requests in
/// flight then finish: `pcx.v1`, the host's `pcx.module.v1` (its module
/// configurations kept by `settings`) and the app's `pcx.views.v1`, behind
/// the same token.
///
/// # Errors
/// The address could not be bound or the server failed.
pub async fn start_grpc_server(
    memory: Arc<Memory>,
    kinds: Arc<dyn post_cortex_ports::DocumentKinds>,
    host: Arc<pcx_host::Host>,
    settings: Option<Arc<dyn pcx_host::Settings>>,
    addr: std::net::SocketAddr,
    auth_token: Option<String>,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    info!("Starting gRPC server on {}", addr);
    tonic::transport::Server::builder()
        .add_routes(crate::compose::grpc_routes(
            memory, kinds, host, settings, auth_token,
        ))
        .serve_with_shutdown(addr, shutdown.cancelled_owned())
        .await
        .map_err(|e| format!("gRPC server: {e}"))
}
