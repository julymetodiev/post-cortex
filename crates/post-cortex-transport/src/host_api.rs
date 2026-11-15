// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The generic gRPC side of the host (`pcx.module.v1.Host`): every module's
//! tools listed as MCP lists them (a module's API is its own services,
//! served by path: `crate::modules`); the running modules listed and their
//! configurations changed and their sandboxes granted
//! ([`pcx_host::ModuleSettings`]); their views listed and read; a module's
//! log read (W25).

use std::sync::Arc;

use pcx_host::{Host, ModuleGrants, ModuleSettings, RootGrant, Settings};
use pcx_module_api::{LogLine, Scope};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::host_server::{Host as HostRpc, HostServer};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::mcp::TOOL_DEADLINE;

/// `pcx.module.v1.Host` over one host.
pub struct HostApi {
    host: Arc<Host>,
    settings: ModuleSettings,
}

impl HostApi {
    /// The service over `host`, whose module configurations `settings`
    /// keeps (none: they are not changed through it).
    #[must_use]
    pub fn new(host: Arc<Host>, settings: Option<Arc<dyn Settings>>) -> Self {
        Self {
            settings: ModuleSettings::new(Arc::clone(&host), settings),
            host,
        }
    }

    /// The tonic server of the service.
    #[must_use]
    pub fn into_server(self) -> HostServer<Self> {
        HostServer::new(self)
    }
}

#[tonic::async_trait]
impl HostRpc for HostApi {
    async fn list_tools(
        &self,
        _request: Request<wire::ListToolsRequest>,
    ) -> Result<Response<wire::ListToolsResponse>, Status> {
        let mut listed = wire::ListToolsResponse::default();
        for tool in self.host.tools() {
            listed
                .rpcs
                .push(self.host.rpc_path(&tool.name).unwrap_or_default());
            listed.tools.push(tool.into());
        }
        Ok(Response::new(listed))
    }

    async fn list_models(
        &self,
        _request: Request<wire::ListModelsRequest>,
    ) -> Result<Response<wire::ListModelsResponse>, Status> {
        Ok(Response::new(wire::ListModelsResponse {
            models: self.host.models(),
        }))
    }

    /// Only a module reports, on its callback socket.
    async fn report(
        &self,
        _request: Request<wire::ReportRequest>,
    ) -> Result<Response<wire::ReportResponse>, Status> {
        Err(Status::permission_denied(
            "Report is a module's, on its callback socket",
        ))
    }

    async fn list_modules(
        &self,
        _request: Request<wire::ListModulesRequest>,
    ) -> Result<Response<wire::ListModulesResponse>, Status> {
        let modules = self
            .host
            .modules()
            .into_iter()
            .map(|m| wire::ModuleInfo::try_from(m.listed()))
            .collect::<Result<_, _>>()
            .map_err(Status::from)?;
        Ok(Response::new(wire::ListModulesResponse {
            modules,
            available: self.settings.available(),
        }))
    }

    async fn get_module(
        &self,
        request: Request<wire::GetModuleRequest>,
    ) -> Result<Response<wire::GetModuleResponse>, Status> {
        let id = request.into_inner().id;
        let module = self
            .host
            .module(&id)
            .map(wire::ModuleInfo::try_from)
            .transpose()
            .map_err(Status::from)?;
        Ok(Response::new(wire::GetModuleResponse { module }))
    }

    async fn module_log(
        &self,
        request: Request<wire::ModuleLogRequest>,
    ) -> Result<Response<wire::ModuleLogResponse>, Status> {
        let id = request.into_inner().id;
        let lines = self
            .host
            .module_log(&id)
            .ok_or_else(|| Status::not_found(format!("no module {id} runs")))?;
        Ok(Response::new(wire::ModuleLogResponse {
            lines: lines.iter().map(LogLine::to_wire).collect(),
        }))
    }

    /// A refused change is in the response; only a malformed request is a
    /// gRPC error.
    async fn configure_module(
        &self,
        request: Request<wire::ConfigureModuleRequest>,
    ) -> Result<Response<wire::ConfigureModuleResponse>, Status> {
        let request = request.into_inner();
        let config = request
            .config
            .ok_or_else(|| Status::invalid_argument("the request has no config"))?;
        let error = self
            .settings
            .configure(&request.id, &config, &request.clear)
            .await
            .err()
            .map(Into::into);
        Ok(Response::new(wire::ConfigureModuleResponse { error }))
    }

    /// A refused grant is in the response.
    async fn grant_module(
        &self,
        request: Request<wire::GrantModuleRequest>,
    ) -> Result<Response<wire::GrantModuleResponse>, Status> {
        let request = request.into_inner();
        let grants = ModuleGrants {
            roots: request
                .roots
                .into_iter()
                .map(|r| RootGrant {
                    path: r.path.into(),
                    write: r.write,
                })
                .collect(),
            network: request.network,
            spawn: request.spawn,
        };
        let error = self
            .settings
            .grant(&request.id, grants)
            .await
            .err()
            .map(Into::into);
        Ok(Response::new(wire::GrantModuleResponse { error }))
    }

    async fn list_views(
        &self,
        _request: Request<wire::ListViewsRequest>,
    ) -> Result<Response<wire::ListViewsResponse>, Status> {
        let views = self
            .host
            .views()
            .into_iter()
            .map(|(module_id, view)| wire::ModuleView {
                module_id,
                view: Some(view),
            })
            .collect();
        Ok(Response::new(wire::ListViewsResponse { views }))
    }

    /// An unknown view, or HTML the module did not serve, is in the
    /// response.
    async fn get_view(
        &self,
        request: Request<wire::GetViewRequest>,
    ) -> Result<Response<wire::GetViewResponse>, Status> {
        let request = request.into_inner();
        let scope = Scope::from_wire(
            wire::Scope::default(),
            CancellationToken::new(),
            TOOL_DEADLINE,
        );
        let answer = match self
            .host
            .view(&scope, &request.module_id, &request.view_id)
            .await
        {
            Ok(read) => wire::GetViewResponse {
                view: Some(read.view),
                resource: read.resource,
                error: None,
                rpcs: read.rpcs,
                descriptors: read.descriptors,
            },
            Err(error) => wire::GetViewResponse {
                view: None,
                resource: None,
                error: Some(error.into()),
                rpcs: Vec::new(),
                descriptors: Vec::new(),
            },
        };
        Ok(Response::new(answer))
    }
}

#[cfg(test)]
mod tests;
