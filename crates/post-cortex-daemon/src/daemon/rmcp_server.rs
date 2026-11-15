// Copyright (c) 2025 Julius ML
// MIT License

//! The daemon's HTTP app: rmcp's Streamable HTTP service on `/mcp` and the
//! REST routes of `post_cortex_transport::rest` on `/api/*` and `/health`,
//! behind the host policy and the token (the daemon starts it in
//! [`crate::daemon::bootstrap`]).

use crate::daemon::host_allowlist::{HostPolicy, require_allowed_host};
use axum::Router;
use pcx_host::Host;
use post_cortex_memory::Memory;
use post_cortex_transport::mcp::McpService;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Builds the HTTP app: rmcp's Streamable HTTP service on `/mcp`, the REST
/// API on `/api/*` and `/health`.
///
/// `host_policy` is enforced by rmcp on `/mcp` and by
/// [`require_allowed_host`] on `/api/*`; `/health` accepts any `Host`. With
/// `auth_token` set every route but `/health` also needs the bearer token.
///
/// The MCP tools are the tools of `host`'s modules; REST acts on `memory`
/// (the memory module's instance).
pub fn build_router(
    host: Arc<Host>,
    memory: Arc<Memory>,
    host_policy: &HostPolicy,
    auth_token: Option<String>,
    ct: &CancellationToken,
) -> Router {
    let mcp_host = host.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(McpService::new(mcp_host.clone(), MCP_INSTRUCTIONS)),
        LocalSessionManager::default().into(),
        host_policy
            .apply(StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token())),
    );

    // A request that carries a reach is held to it (docs/connect.md 4a).
    let api_router = post_cortex_transport::rest::routes(Arc::clone(&memory), host)
        .layer(axum::middleware::from_fn_with_state(
            memory,
            post_cortex_transport::reach::rest,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(host_policy.clone()),
            require_allowed_host,
        ));

    let router = Router::new()
        .nest_service("/mcp", mcp_service)
        .merge(api_router);
    match auth_token {
        Some(token) => router.layer(axum::middleware::from_fn_with_state(
            Arc::new(token),
            crate::daemon::auth::require_bearer,
        )),
        None => router,
    }
}

/// What the MCP server tells its clients.
const MCP_INSTRUCTIONS: &str = "Post-Cortex: Intelligent conversation memory system with 9 consolidated MCP tools. \
     Tools: session, update_conversation_context, semantic_search, get_structured_summary, \
     query_conversation_context, manage_workspace, assemble_context, manage_entity, admin. \
     All use shared RocksDB for centralized knowledge management.";

#[cfg(test)]
mod events_tests;
#[cfg(test)]
mod route_tests;
#[cfg(test)]
mod tests;
