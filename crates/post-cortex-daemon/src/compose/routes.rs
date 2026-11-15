// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The routes a memory instance serves (ARCHITECTURE W9): MCP and REST over
//! the host and memory and, for browsers, gRPC (`pcx.v1`, `pcx.views.v1`,
//! `pcx.module.v1`) with gRPC-Web and the browser transfer
//! (`post_cortex_transport::browser`). The daemon and every hosted tenant
//! serve these, so a tenant names no transport internals.

use post_cortex_ports::DocumentKinds;
use std::sync::Arc;

use axum::Router;
use pcx_host::{Host, Origin, Settings};
use pcx_module_api::agent::TurnsService;
use post_cortex_memory::Memory;
use post_cortex_proto::agent::agent_server::AgentServer;
use post_cortex_proto::module::host_server::HostServer;
use post_cortex_proto::pb::blobs_server::BlobsServer;
use post_cortex_proto::pb::post_cortex_server::PostCortexServer;
use post_cortex_proto::views::views_server::ViewsServer;
use post_cortex_transport::agent::AgentService;
use post_cortex_transport::browser::transfer::{
    BrowserTransfer, DEFAULT_LINK_BASE, DEFAULT_LINK_TTL,
};
use post_cortex_transport::browser::{self, Origins};
use post_cortex_transport::host_api::HostApi;
use post_cortex_transport::job_health;
use post_cortex_transport::modules;
use post_cortex_transport::reach::Reached;
use post_cortex_transport::views::ViewsService;
use post_cortex_transport::webhooks;
use tokio_util::sync::CancellationToken;
use tonic::service::interceptor::InterceptedService;

use crate::daemon::host_allowlist::{HostPolicy, require_allowed_host};
use post_cortex_transport::memory::{Build, PcxGrpcService};

/// Whether, and for whom, an instance serves browsers.
#[derive(Debug, Clone)]
pub enum BrowserAccess {
    /// No browser routes (a daemon with no configured origin).
    Off,
    /// The browser routes, for these origins (CORS).
    Origins(Origins),
    /// The browser routes without CORS, behind a gateway that answers
    /// browsers itself; download links point under `link_base`, the path
    /// the gateway routes to this instance without a credential.
    Proxied {
        /// Where links point (`<link_base>/export/<ticket>`).
        link_base: String,
    },
}

/// How an instance serves its routes.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    /// The `Host` headers accepted (DNS-rebinding guard).
    pub host_policy: HostPolicy,
    /// The bearer token every route but `/health` (and a download link)
    /// needs; `None`: open (a loopback daemon, or a tenant behind its
    /// gateway).
    pub auth_token: Option<String>,
    /// Browser access.
    pub browser: BrowserAccess,
}

/// The build `Health` names: this binary's version and commit.
pub const BUILD: Build = Build {
    version: env!("CARGO_PKG_VERSION"),
    commit: env!("PCX_COMMIT"),
};

/// The gRPC services of `memory` and `host`: `pcx.v1`, `pcx.views.v1`,
/// `pcx.module.v1` (module configurations kept by `settings`; none: not
/// changed through it, as for a hosted tenant), `pcx.agent.v1` (the
/// host's agent, when one runs) and, on every other path, the running
/// modules' services (W23), behind `auth_token` when one is set.
pub fn grpc_routes(
    memory: Arc<Memory>,
    kinds: Arc<dyn DocumentKinds>,
    host: Arc<Host>,
    settings: Option<Arc<dyn Settings>>,
    auth_token: Option<String>,
) -> tonic::service::Routes {
    let views = ViewsService::new(Arc::clone(&memory));
    // Memory refuses a call outside its reach (a connected daemon's tunnel,
    // a gateway's restricted grant: docs/connect.md).
    let reached = Arc::clone(&memory);
    let service = PcxGrpcService::new(memory, BUILD, kinds);
    let agent = AgentService::new(Arc::clone(&host));
    let channel = host.channel(Origin::Client);
    let invoke = HostApi::new(host, settings);
    let (routes, modules) = match auth_token {
        Some(token) => {
            let check = crate::daemon::auth::grpc_interceptor(Arc::new(token));
            let modules = modules::service(channel, check.clone());
            let routes = tonic::service::Routes::new(InterceptedService::new(
                Reached::new(PostCortexServer::new(service.clone()), Arc::clone(&reached)),
                check.clone(),
            ))
            .add_service(InterceptedService::new(
                Reached::new(BlobsServer::new(service), Arc::clone(&reached)),
                check.clone(),
            ))
            .add_service(HostServer::with_interceptor(invoke, check.clone()))
            .add_service(AgentServer::with_interceptor(
                TurnsService::new(agent),
                check.clone(),
            ))
            .add_service(InterceptedService::new(
                Reached::new(ViewsServer::new(views), reached),
                check,
            ));
            (routes, modules)
        }
        None => {
            let check = crate::daemon::auth::from_client;
            let modules = modules::service(channel, check);
            let routes = tonic::service::Routes::new(InterceptedService::new(
                Reached::new(PostCortexServer::new(service.clone()), Arc::clone(&reached)),
                check,
            ))
            .add_service(InterceptedService::new(
                Reached::new(BlobsServer::new(service), Arc::clone(&reached)),
                check,
            ))
            .add_service(HostServer::with_interceptor(invoke, check))
            .add_service(AgentServer::with_interceptor(
                TurnsService::new(agent),
                check,
            ))
            .add_service(InterceptedService::new(
                Reached::new(ViewsServer::new(views), reached),
                check,
            ));
            (routes, modules)
        }
    };
    tonic::service::Routes::from(routes.into_axum_router().fallback_service(modules))
}

/// The routes of the instance over `memory` and `host` (see the module
/// docs; `settings` as for [`grpc_routes`]), as `config` says.
pub fn routes(
    host: Arc<Host>,
    memory: Arc<Memory>,
    kinds: Arc<dyn DocumentKinds>,
    settings: Option<Arc<dyn Settings>>,
    config: RouteConfig,
    ct: &CancellationToken,
) -> Router {
    // The webhooks authenticate with their own token, not the instance's,
    // and a job's health is read by a probe that carries none.
    let hooks = webhooks::routes(host.channel(Origin::Client))
        .merge(job_health::routes(host.channel(Origin::Client)))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(config.host_policy.clone()),
            require_allowed_host,
        ));
    let api = crate::daemon::rmcp_server::build_router(
        Arc::clone(&host),
        Arc::clone(&memory),
        &config.host_policy,
        config.auth_token.clone(),
        ct,
    )
    .merge(hooks);
    let (origins, link_base) = match config.browser {
        BrowserAccess::Off => return api,
        BrowserAccess::Origins(origins) => (Some(origins), DEFAULT_LINK_BASE.to_string()),
        BrowserAccess::Proxied { link_base } => (None, link_base),
    };
    let transfer = BrowserTransfer::new(Arc::clone(&memory), DEFAULT_LINK_TTL, link_base);
    // An upload that carries a reach is held to it (docs/connect.md 4a).
    let uploads = browser::transfer::routes(Arc::clone(&transfer)).layer(
        axum::middleware::from_fn_with_state(
            Arc::clone(&memory),
            post_cortex_transport::reach::browser,
        ),
    );
    let uploads = match &config.auth_token {
        Some(token) => uploads.layer(axum::middleware::from_fn_with_state(
            Arc::new(token.clone()),
            crate::daemon::auth::require_bearer,
        )),
        None => uploads,
    };
    let browser_routes = grpc_routes(memory, kinds, host, settings, config.auth_token)
        .into_axum_router()
        .layer(tonic_web::GrpcWebLayer::new())
        .merge(uploads)
        .merge(browser::transfer::downloads(transfer))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(config.host_policy),
            require_allowed_host,
        ));
    let router = api.merge(browser_routes);
    match origins {
        Some(origins) => router.layer(browser::cors(origins)),
        None => router,
    }
}
