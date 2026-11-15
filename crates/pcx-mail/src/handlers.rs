// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The mail module's `pcx.mail.v1.Mail` service: each RPC maps its request
//! to the service and its answer or failure back.

use pcx_module_api::Scope;
use post_cortex_proto::mail::mail_server::Mail;
use post_cortex_proto::mail::{
    Filled, GetTemplateRequest, ListLogRequest, ListTemplatesRequest, LogPage,
    PreviewTemplateRequest, SaveTemplateRequest, SendRequest, SentRecord, TemplateList,
    TemplateRecord, TestRequest,
};
use tonic::{Request, Response, Status};

use crate::module::{Shared, running};

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

#[tonic::async_trait]
impl Mail for Handlers {
    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SentRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let sent = running
            .service(&docs)
            .send(request.into_inner(), scope.author.as_deref())
            .await?;
        Ok(Response::new(sent))
    }

    async fn test(&self, request: Request<TestRequest>) -> Result<Response<SentRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let sent = running
            .service(&docs)
            .test(&request.into_inner().to, scope.author.as_deref())
            .await?;
        Ok(Response::new(sent))
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

    async fn list_templates(
        &self,
        request: Request<ListTemplatesRequest>,
    ) -> Result<Response<TemplateList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        Ok(Response::new(
            running.service(&docs).list_templates().await?,
        ))
    }

    async fn get_template(
        &self,
        request: Request<GetTemplateRequest>,
    ) -> Result<Response<TemplateRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let template = running
            .service(&docs)
            .get_template(&request.into_inner().name)
            .await?;
        Ok(Response::new(template))
    }

    async fn save_template(
        &self,
        request: Request<SaveTemplateRequest>,
    ) -> Result<Response<TemplateRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let saved = running
            .service(&docs)
            .save_template(request.into_inner())
            .await?;
        Ok(Response::new(saved))
    }

    async fn preview_template(
        &self,
        request: Request<PreviewTemplateRequest>,
    ) -> Result<Response<Filled>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let filled = running
            .service(&docs)
            .preview_template(request.into_inner())
            .await?;
        Ok(Response::new(filled))
    }
}
