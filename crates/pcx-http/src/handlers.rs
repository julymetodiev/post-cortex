// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The http module's `pcx.http.v1.Http` service: each RPC maps its request
//! (and its idempotency key) to the service and its answer or failure
//! back.

use pcx_module_api::{ModuleError, ModuleErrorKind, Scope, metadata};
use post_cortex_proto::http::http_server::Http;
use post_cortex_proto::http::{
    ConnectionList, DeletePresetRequest, Deleted, GetPresetRequest, HttpRequest, HttpResponse,
    ListConnectionsRequest, ListLogRequest, ListPresetsRequest, LogPage, PresetList, PresetRecord,
    SavePresetRequest, TestConnectionRequest, Tested,
};
use tonic::{Request, Response, Status};

use crate::module::{Shared, running};

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

/// The idempotency key `request` carries.
fn key<T>(request: &Request<T>) -> Result<Option<String>, ModuleError> {
    request
        .metadata()
        .get(metadata::IDEMPOTENCY_KEY)
        .map(|value| {
            value.to_str().map(str::to_string).map_err(|_| {
                ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    "the idempotency key is not ASCII text",
                )
            })
        })
        .transpose()
}

impl Handlers {
    async fn call(
        &self,
        request: Request<HttpRequest>,
        reads: bool,
    ) -> Result<Response<HttpResponse>, Status> {
        let scope = Scope::of(&request)?;
        let key = key(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let answered = running
            .service(&docs)
            .call(
                request.into_inner(),
                reads,
                scope.author.as_deref(),
                key.as_deref(),
            )
            .await?;
        Ok(Response::new(answered))
    }
}

#[tonic::async_trait]
impl Http for Handlers {
    async fn get(&self, request: Request<HttpRequest>) -> Result<Response<HttpResponse>, Status> {
        self.call(request, true).await
    }

    async fn request(
        &self,
        request: Request<HttpRequest>,
    ) -> Result<Response<HttpResponse>, Status> {
        self.call(request, false).await
    }

    async fn list_connections(
        &self,
        request: Request<ListConnectionsRequest>,
    ) -> Result<Response<ConnectionList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        Ok(Response::new(running.service(&docs).connections()))
    }

    async fn test_connection(
        &self,
        request: Request<TestConnectionRequest>,
    ) -> Result<Response<Tested>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let tested = running
            .service(&docs)
            .test(&request.into_inner().connection, scope.author.as_deref())
            .await?;
        Ok(Response::new(tested))
    }

    async fn list_presets(
        &self,
        request: Request<ListPresetsRequest>,
    ) -> Result<Response<PresetList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let listed = running
            .service(&docs)
            .list_presets(request.get_ref())
            .await?;
        Ok(Response::new(listed))
    }

    async fn get_preset(
        &self,
        request: Request<GetPresetRequest>,
    ) -> Result<Response<PresetRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let preset = running
            .service(&docs)
            .get_preset(&request.get_ref().name)
            .await?;
        Ok(Response::new(preset))
    }

    async fn save_preset(
        &self,
        request: Request<SavePresetRequest>,
    ) -> Result<Response<PresetRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let saved = running
            .service(&docs)
            .save_preset(request.into_inner())
            .await?;
        Ok(Response::new(saved))
    }

    async fn list_log(
        &self,
        request: Request<ListLogRequest>,
    ) -> Result<Response<LogPage>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let page = running
            .service(&docs)
            .list_log(request.into_inner())
            .await?;
        Ok(Response::new(page))
    }

    async fn delete_preset(
        &self,
        request: Request<DeletePresetRequest>,
    ) -> Result<Response<Deleted>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let asked = request.get_ref();
        let deleted = running
            .service(&docs)
            .delete_preset(&asked.name, asked.version)
            .await?;
        Ok(Response::new(deleted))
    }
}
