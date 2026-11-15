// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The module's services: `pcx.channel.v1.Channel` and
//! `pcx.discord.v1.Discord`. Each handler maps its request to the service
//! and its answer or failure back.

use post_cortex_proto::channel::channel_server::Channel;
use post_cortex_proto::channel::{SendRequest, SendResponse};
use post_cortex_proto::discord::discord_server::Discord;
use post_cortex_proto::discord::{
    BanRequest, BanResponse, GiveRoleRequest, GiveRoleResponse, LockThreadRequest,
    LockThreadResponse, PostRequest, Posted, ReplyRequest, ServerStatus, StatusRequest,
    TagPostRequest, TagPostResponse, TimeoutRequest, TimeoutResponse,
};
use tonic::{Request, Response, Status};

use crate::module::{Shared, running};

/// The services' handlers.
#[derive(Clone)]
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

#[tonic::async_trait]
impl Channel for Handlers {
    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        let message_id = running
            .service()?
            .send(&request.destination, &request.text)
            .await?;
        Ok(Response::new(SendResponse { message_id }))
    }
}

#[tonic::async_trait]
impl Discord for Handlers {
    async fn post(&self, request: Request<PostRequest>) -> Result<Response<Posted>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        let message_id = running
            .service()?
            .post(&request.channel, &request.text, request.embed)
            .await?;
        Ok(Response::new(Posted { message_id }))
    }

    async fn reply(&self, request: Request<ReplyRequest>) -> Result<Response<Posted>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        let message_id = running
            .service()?
            .reply(
                &request.thread_id,
                &request.channel,
                &request.message_id,
                &request.text,
            )
            .await?;
        Ok(Response::new(Posted { message_id }))
    }

    async fn give_role(
        &self,
        request: Request<GiveRoleRequest>,
    ) -> Result<Response<GiveRoleResponse>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        running
            .service()?
            .give_role(&request.member_id, &request.role)
            .await?;
        Ok(Response::new(GiveRoleResponse {}))
    }

    async fn tag_post(
        &self,
        request: Request<TagPostRequest>,
    ) -> Result<Response<TagPostResponse>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        running
            .service()?
            .tag_post(&request.thread_id, &request.tag)
            .await?;
        Ok(Response::new(TagPostResponse {}))
    }

    async fn timeout(
        &self,
        request: Request<TimeoutRequest>,
    ) -> Result<Response<TimeoutResponse>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        let until = running
            .service()?
            .time_out(&request.member_id, request.minutes)
            .await?;
        Ok(Response::new(TimeoutResponse {
            until: until.unwrap_or_default(),
        }))
    }

    async fn lock_thread(
        &self,
        request: Request<LockThreadRequest>,
    ) -> Result<Response<LockThreadResponse>, Status> {
        let running = running(&self.shared)?;
        running
            .service()?
            .lock_thread(&request.into_inner().thread_id)
            .await?;
        Ok(Response::new(LockThreadResponse {}))
    }

    async fn ban(&self, request: Request<BanRequest>) -> Result<Response<BanResponse>, Status> {
        let running = running(&self.shared)?;
        let request = request.into_inner();
        running
            .service()?
            .ban(&request.member_id, request.delete_message_hours)
            .await?;
        Ok(Response::new(BanResponse {}))
    }

    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<ServerStatus>, Status> {
        Ok(Response::new(running(&self.shared)?.status()))
    }
}
