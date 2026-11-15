// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The callback socket of a module process (ARCHITECTURE section 7,
//! modules together, sandbox and tools): `Host.Invoke` and
//! `Host.ListModules` (manifests only) answered through the module's handle
//! on the host, so a process module calls the tools of the modules it uses
//! as an in-process one does (`Host.ListTools`: the tools it may call),
//! and its `Sandbox`, `Model`, `Hooks`, `ContextEngine` and
//! `pcx.agent.v1.Agent` services, each through the handle's checks (the
//! models and context for their slots, the agent's turns for a module that
//! uses the agent). The host's other RPCs are not a module's to call.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use pcx_module_api::agent::TurnsService;
use pcx_module_api::bus::BusService;
use pcx_module_api::context::ContextService;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::model::ModelService;
use pcx_module_api::{DEFAULT_DEADLINE, ModuleHost, Scope, ThroughHost};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_server::HooksServer;
use post_cortex_proto::module::host_server::{Host as HostService, HostServer};
use post_cortex_proto::module::sandbox_server::SandboxServer;
use tokio::net::UnixListener;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnixListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

/// A running callback server.
pub struct Callback {
    stop: CancellationToken,
    server: JoinHandle<()>,
}

impl Callback {
    /// Serve `host` (the module `module`'s handle) on the Unix socket
    /// `socket`, and `channel` (its calls to the modules' gRPC services,
    /// W23) for every call the host's own services do not take.
    ///
    /// # Errors
    /// The socket cannot be bound.
    pub fn serve(
        host: Arc<dyn ModuleHost>,
        channel: GrpcChannel,
        socket: &Path,
        module: &str,
    ) -> Result<Self, String> {
        let listener = UnixListener::bind(socket)
            .map_err(|e| format!("bind the callback socket {}: {e}", socket.display()))?;
        // A sandboxed module may run as another user: its directory, the
        // launch's own, is what guards this socket.
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o666))
            .map_err(|e| format!("open the callback socket {}: {e}", socket.display()))?;
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let module = module.to_string();
        let sandbox = crate::process::sandbox_service::Served::new(Arc::clone(&host));
        let through = ThroughHost::new(Arc::clone(&host));
        let models = ModelService::new(through.clone()).into_server();
        let context = ContextService::new(through.clone()).into_server();
        let bus = BusService::new(through.clone()).into_server();
        let turns = TurnsService::new(through).into_server();
        let hooks = crate::process::hooks_service::Hooks::new(Arc::clone(&host));
        let mut builder = tonic::service::Routes::builder();
        builder
            .add_service(HostServer::new(Served { host }))
            .add_service(SandboxServer::new(sandbox))
            .add_service(models)
            .add_service(HooksServer::new(hooks))
            .add_service(context)
            .add_service(bus)
            .add_service(turns);
        let routes = grpc::with_fallback(builder.routes(), channel);
        let server = tokio::spawn(async move {
            let served = tonic::transport::Server::builder()
                .add_routes(routes)
                .serve_with_incoming_shutdown(
                    UnixListenerStream::new(listener),
                    stopped.cancelled(),
                )
                .await;
            if let Err(e) = served {
                tracing::warn!(module, error = %e, "the module's callback server failed");
            }
        });
        Ok(Self { stop, server })
    }

    /// Stop serving at once, without waiting (a module dropped unstopped).
    pub fn abort(self) {
        self.stop.cancel();
        self.server.abort();
    }

    /// Stop serving and wait for the server.
    pub async fn stop(self, module: &str) {
        self.stop.cancel();
        if let Err(e) = self.server.await {
            tracing::warn!(module, error = %e, "the module's callback server");
        }
    }
}

/// The module's handle behind the generated service.
struct Served {
    host: Arc<dyn ModuleHost>,
}

fn not_for_modules(rpc: &str) -> Status {
    Status::permission_denied(format!(
        "{rpc} is not served on a module's callback socket (ListTools, ListModules and Report are)"
    ))
}

#[tonic::async_trait]
impl HostService for Served {
    /// The tools of the modules this one uses, for the calls of the
    /// request's scope.
    async fn list_tools(
        &self,
        request: Request<wire::ListToolsRequest>,
    ) -> Result<Response<wire::ListToolsResponse>, Status> {
        let scope = request
            .into_inner()
            .scope
            .ok_or_else(|| Status::invalid_argument("the request has no scope"))?;
        let scope = Scope::from_wire(scope, CancellationToken::new(), DEFAULT_DEADLINE);
        let used = self.host.tools(&scope).await.map_err(Status::from)?;
        let mut listed = wire::ListToolsResponse::default();
        for used in used {
            listed.rpcs.push(used.tool.path().to_string());
            listed.tools.push(used.tool.spec.into());
            listed.modules.push(used.module);
        }
        Ok(Response::new(listed))
    }

    async fn list_modules(
        &self,
        _request: Request<wire::ListModulesRequest>,
    ) -> Result<Response<wire::ListModulesResponse>, Status> {
        let manifests = self.host.manifests().await.map_err(Status::from)?;
        let modules = manifests
            .into_iter()
            .map(|manifest| wire::ModuleInfo {
                manifest: Some(manifest),
                ..wire::ModuleInfo::default()
            })
            .collect();
        Ok(Response::new(wire::ListModulesResponse {
            modules,
            available: Vec::new(),
        }))
    }

    async fn configure_module(
        &self,
        _request: Request<wire::ConfigureModuleRequest>,
    ) -> Result<Response<wire::ConfigureModuleResponse>, Status> {
        Err(not_for_modules("ConfigureModule"))
    }

    async fn grant_module(
        &self,
        _request: Request<wire::GrantModuleRequest>,
    ) -> Result<Response<wire::GrantModuleResponse>, Status> {
        Err(not_for_modules("GrantModule"))
    }

    async fn list_views(
        &self,
        _request: Request<wire::ListViewsRequest>,
    ) -> Result<Response<wire::ListViewsResponse>, Status> {
        Err(not_for_modules("ListViews"))
    }

    async fn get_view(
        &self,
        _request: Request<wire::GetViewRequest>,
    ) -> Result<Response<wire::GetViewResponse>, Status> {
        Err(not_for_modules("GetView"))
    }

    async fn module_log(
        &self,
        _request: Request<wire::ModuleLogRequest>,
    ) -> Result<Response<wire::ModuleLogResponse>, Status> {
        Err(not_for_modules("ModuleLog"))
    }

    async fn get_module(
        &self,
        _request: Request<wire::GetModuleRequest>,
    ) -> Result<Response<wire::GetModuleResponse>, Status> {
        Err(not_for_modules("GetModule"))
    }

    async fn list_models(
        &self,
        _request: Request<wire::ListModelsRequest>,
    ) -> Result<Response<wire::ListModelsResponse>, Status> {
        Err(not_for_modules("ListModels"))
    }

    /// A refusal is in the answer.
    async fn report(
        &self,
        request: Request<wire::ReportRequest>,
    ) -> Result<Response<wire::ReportResponse>, Status> {
        let error = self
            .host
            .report(&request.into_inner().problem)
            .await
            .err()
            .map(Into::into);
        Ok(Response::new(wire::ReportResponse { error }))
    }
}

#[cfg(test)]
mod tests;
