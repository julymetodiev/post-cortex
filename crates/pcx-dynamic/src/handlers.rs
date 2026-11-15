// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx.dynamic.v1.Dynamic` service: each RPC maps its request to the
//! service and its answer or failure back.

use pcx_module_api::Scope;
use post_cortex_proto::dynamic::dynamic_server::Dynamic;
use post_cortex_proto::dynamic::{
    ApprovedList, DynamicList, DynamicRecord, EditRequest, FilesRequest, GetRequest,
    InstallRequest, ListApprovedRequest, ListRequest, ModuleFiles, RemoveRequest, RemoveResponse,
};
use tonic::{Request, Response, Status};

use crate::module::{Shared, running};

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

#[tonic::async_trait]
impl Dynamic for Handlers {
    async fn install(
        &self,
        request: Request<InstallRequest>,
    ) -> Result<Response<DynamicRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let installed = running
            .service(&docs)
            .install(&scope, request.into_inner())
            .await?;
        Ok(Response::new(installed))
    }

    async fn list(&self, request: Request<ListRequest>) -> Result<Response<DynamicList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        Ok(Response::new(running.service(&docs).list().await?))
    }

    async fn get(&self, request: Request<GetRequest>) -> Result<Response<DynamicRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let found = running.service(&docs).get(&request.get_ref().id).await?;
        Ok(Response::new(found))
    }

    async fn edit(&self, request: Request<EditRequest>) -> Result<Response<DynamicRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let edited = running
            .service(&docs)
            .edit(&scope, request.into_inner())
            .await?;
        Ok(Response::new(edited))
    }

    async fn files(&self, request: Request<FilesRequest>) -> Result<Response<ModuleFiles>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let files = running
            .service(&docs)
            .module_files(&request.get_ref().id)
            .await?;
        Ok(Response::new(files))
    }

    async fn remove(
        &self,
        request: Request<RemoveRequest>,
    ) -> Result<Response<RemoveResponse>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        running.service(&docs).remove(&request.get_ref().id).await?;
        Ok(Response::new(RemoveResponse {}))
    }

    async fn list_approved(
        &self,
        request: Request<ListApprovedRequest>,
    ) -> Result<Response<ApprovedList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        Ok(Response::new(running.service(&docs).approved().await?))
    }
}
