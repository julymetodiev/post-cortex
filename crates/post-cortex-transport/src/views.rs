// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx.views.v1.Views` service over a memory instance's detail views
//! (`Memory::detail_views`): every handler parses its request
//! ([`request`]), calls one capability method and maps the result
//! ([`response`]) or the failure ([`crate::grpc::status`]).

use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::EntityRequest;
use post_cortex_memory::graph_views::detail::PageAt;
use post_cortex_proto::views as pb;
use post_cortex_proto::views::views_server::{Views, ViewsServer};
use tonic::{Request, Response, Status};

use crate::grpc::status;

pub mod request;
pub mod response;
pub mod streams;

/// The `Views` service over one memory instance.
pub struct ViewsService {
    memory: Arc<Memory>,
}

impl ViewsService {
    /// The service over `memory`.
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }

    /// The tonic server for this service.
    pub fn into_server(self) -> ViewsServer<Self> {
        ViewsServer::new(self)
    }
}

#[tonic::async_trait]
impl Views for ViewsService {
    async fn get_graph(
        &self,
        req: Request<pb::GetGraphRequest>,
    ) -> Result<Response<pb::Graph>, Status> {
        let req = req.into_inner();
        let at = request::time(req.at)?;
        let target = request::target(req.scope)?;
        let graph = self
            .memory
            .detail_views
            .graph(target, request::count(req.limit), at)
            .await
            .map_err(status)?;
        Ok(Response::new(response::graph(graph)))
    }

    async fn get_timeline(
        &self,
        req: Request<pb::GetTimelineRequest>,
    ) -> Result<Response<pb::Timeline>, Status> {
        let req = req.into_inner();
        let (from, to) = (request::time(req.from)?, request::time(req.to)?);
        let target = request::target(req.scope)?;
        let timeline = self
            .memory
            .detail_views
            .timeline(target, from, to, request::count(req.limit))
            .await
            .map_err(status)?;
        Ok(Response::new(response::timeline(timeline)))
    }

    async fn list_records(
        &self,
        req: Request<pb::ListRecordsRequest>,
    ) -> Result<Response<pb::RecordPage>, Status> {
        let req = req.into_inner();
        let (filter, cursor) = request::record_filter(&req)?;
        let target = request::target(req.scope)?;
        let page = self
            .memory
            .detail_views
            .records(target, filter, request::count(req.limit), cursor)
            .await
            .map_err(status)?;
        Ok(Response::new(response::record_page(page)))
    }

    async fn get_record(
        &self,
        req: Request<pb::GetRecordRequest>,
    ) -> Result<Response<pb::Record>, Status> {
        let req = req.into_inner();
        let id = request::uuid(&req.id, "id")?;
        let target = request::target(req.scope)?;
        let record = self
            .memory
            .detail_views
            .record(target, id)
            .await
            .map_err(status)?;
        Ok(Response::new(response::record(record)))
    }

    async fn get_records(
        &self,
        req: Request<pb::GetRecordsRequest>,
    ) -> Result<Response<pb::Records>, Status> {
        let req = req.into_inner();
        let ids = req
            .ids
            .iter()
            .map(|id| request::uuid(id, "ids"))
            .collect::<Result<Vec<_>, _>>()?;
        let target = request::target(req.scope)?;
        let records = self
            .memory
            .detail_views
            .records_of(target, ids)
            .await
            .map_err(status)?;
        Ok(Response::new(pb::Records {
            records: records.into_iter().map(response::record).collect(),
        }))
    }

    async fn get_overview(
        &self,
        req: Request<pb::GetOverviewRequest>,
    ) -> Result<Response<pb::Overview>, Status> {
        let req = req.into_inner();
        let target = request::target(req.scope)?;
        let overview = self
            .memory
            .detail_views
            .overview(
                target,
                request::count(req.top_entities),
                request::count(req.recent),
            )
            .await
            .map_err(status)?;
        Ok(Response::new(response::overview(overview)))
    }

    async fn get_entity(
        &self,
        req: Request<pb::GetEntityRequest>,
    ) -> Result<Response<pb::EntityDetail>, Status> {
        let req = req.into_inner();
        let target = request::target(req.scope)?;
        let entity = self
            .memory
            .detail_views
            .entity(
                target,
                EntityRequest {
                    name: req.name,
                    limit: request::count(req.limit),
                    impact_depth: request::count(req.impact_depth),
                    lineage_depth: request::count(req.lineage_depth),
                },
            )
            .await
            .map_err(status)?;
        Ok(Response::new(response::entity(entity)))
    }

    async fn search(
        &self,
        req: Request<pb::SearchRequest>,
    ) -> Result<Response<pb::SearchResponse>, Status> {
        let req = req.into_inner();
        let options = request::search_options(&req)?;
        let target = request::target(req.scope)?;
        let found = self
            .memory
            .detail_views
            .search(target, &req.query, options)
            .await
            .map_err(status)?;
        Ok(Response::new(response::search(found)?))
    }

    async fn list_topics(
        &self,
        req: Request<pb::ListTopicsRequest>,
    ) -> Result<Response<pb::Topics>, Status> {
        let req = req.into_inner();
        let target = request::target(req.scope)?;
        let topics = self
            .memory
            .detail_views
            .topics(
                target,
                request::count(req.limit),
                request::count(req.per_topic),
            )
            .await
            .map_err(status)?;
        Ok(Response::new(response::topics(topics)))
    }

    async fn get_radar(
        &self,
        req: Request<pb::GetRadarRequest>,
    ) -> Result<Response<pb::Radar>, Status> {
        let req = req.into_inner();
        let target = request::target(req.scope)?;
        let radar = self
            .memory
            .detail_views
            .radar(target, request::count(req.limit), req.include_dismissed)
            .await
            .map_err(status)?;
        Ok(Response::new(response::radar(radar)))
    }

    async fn get_file_view(
        &self,
        req: Request<pb::GetFileViewRequest>,
    ) -> Result<Response<pb::FileView>, Status> {
        let req = req.into_inner();
        let target = request::target(req.scope)?;
        let file = self
            .memory
            .detail_views
            .file(
                target,
                req.path,
                request::count(req.depth),
                request::count(req.limit),
            )
            .await
            .map_err(status)?;
        Ok(Response::new(response::file(file)))
    }

    async fn list_files(
        &self,
        req: Request<pb::ListFilesRequest>,
    ) -> Result<Response<pb::Files>, Status> {
        let req = req.into_inner();
        let target = request::target(req.scope)?;
        let files = self
            .memory
            .detail_views
            .files(
                target,
                req.text,
                request::count(req.limit),
                request::cursor(&req.cursor)?,
            )
            .await
            .map_err(status)?;
        Ok(Response::new(response::files(files)))
    }

    async fn list_conversations(
        &self,
        req: Request<pb::ListConversationsRequest>,
    ) -> Result<Response<pb::Conversations>, Status> {
        let req = req.into_inner();
        let target = request::target(req.scope)?;
        let list = self
            .memory
            .detail_views
            .conversations(
                target,
                request::count(req.limit),
                request::cursor(&req.cursor)?,
                req.speaker.as_deref(),
            )
            .await
            .map_err(status)?;
        Ok(Response::new(streams::conversations(list)))
    }

    async fn get_conversation(
        &self,
        req: Request<pb::GetConversationRequest>,
    ) -> Result<Response<pb::Conversation>, Status> {
        let req = req.into_inner();
        let session = request::uuid(&req.session_id, "session_id")?;
        let at = match req.before {
            Some(seq) => PageAt::Before(seq),
            None => PageAt::From(req.cursor),
        };
        let page = self
            .memory
            .detail_views
            .conversation(session, at, request::count(req.limit))
            .await
            .map_err(status)?;
        Ok(Response::new(streams::conversation(page)?))
    }

    type WatchActivityStream =
        Pin<Box<dyn Stream<Item = Result<pb::ActivityEvent, Status>> + Send>>;

    async fn watch_activity(
        &self,
        req: Request<pb::WatchActivityRequest>,
    ) -> Result<Response<Self::WatchActivityStream>, Status> {
        let (target, request) = request::watch(req.into_inner())?;
        let events = self
            .memory
            .detail_views
            .watch(target, request)
            .await
            .map_err(status)?;
        Ok(Response::new(Box::pin(events.map(|event| {
            event.map_err(status).and_then(streams::activity)
        }))))
    }
}
