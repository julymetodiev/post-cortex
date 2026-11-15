// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The `pcx.v1` gRPC service of memory (ARCHITECTURE section 3: transports
//! are thin mappings). The `impl PostCortex` block only delegates: each
//! method's body is a function of the module of its concern, over the
//! memory (sessions, records, search, views, assembly, workspaces,
//! lifecycle, projects, capture, freshness); `helpers` parses and maps.

use post_cortex_proto::pb;
use post_cortex_proto::pb::post_cortex_server::PostCortex;
use tonic::{Request, Response, Status};

use pb::*;

mod analysis;
mod assembly;
mod blobs;
mod capture;
mod documents;
mod freshness;
mod helpers;
mod lifecycle;
mod projects;
mod records;
mod search;
mod service;
mod sessions;
mod views;
mod workspaces;

pub use service::{Build, PcxGrpcService};

#[tonic::async_trait]
impl PostCortex for PcxGrpcService {
    async fn put_document(
        &self,
        request: Request<PutDocumentRequest>,
    ) -> Result<Response<PutDocumentResponse>, Status> {
        documents::put(self.memory(), self.kinds(), request).await
    }

    async fn get_document(
        &self,
        request: Request<GetDocumentRequest>,
    ) -> Result<Response<Document>, Status> {
        documents::get(self.memory(), request).await
    }

    async fn get_document_history(
        &self,
        request: Request<GetDocumentHistoryRequest>,
    ) -> Result<Response<DocumentHistory>, Status> {
        documents::history(self.memory(), request).await
    }

    async fn list_documents(
        &self,
        request: Request<ListDocumentsRequest>,
    ) -> Result<Response<DocumentPage>, Status> {
        documents::list(self.memory(), self.kinds(), request).await
    }

    async fn delete_document(
        &self,
        request: Request<DeleteDocumentRequest>,
    ) -> Result<Response<DeleteDocumentResponse>, Status> {
        documents::delete(self.memory(), self.kinds(), request).await
    }

    type ExportStream = crate::transfer::ExportStream;

    async fn append_messages(
        &self,
        request: Request<AppendMessagesRequest>,
    ) -> Result<Response<AppendMessagesResponse>, Status> {
        capture::append_messages(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn list_messages(
        &self,
        request: Request<ListMessagesRequest>,
    ) -> Result<Response<MessagePage>, Status> {
        capture::list_messages(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn get_card(
        &self,
        request: Request<GetCardRequest>,
    ) -> Result<Response<GetCardResponse>, Status> {
        let person = crate::person::of_metadata(request.metadata())?;
        capture::get_card(self.memory(), request.into_inner(), person)
            .await
            .map(Response::new)
    }

    async fn update_card(
        &self,
        request: Request<UpdateCardRequest>,
    ) -> Result<Response<UpdateCardResponse>, Status> {
        let person = crate::person::of_metadata(request.metadata())?;
        capture::update_card(self.memory(), request.into_inner(), person)
            .await
            .map(Response::new)
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(self.health_response()))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        sessions::create_session(self.memory(), request).await
    }

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        sessions::list_sessions(self.memory(), request).await
    }

    async fn load_session(
        &self,
        request: Request<LoadSessionRequest>,
    ) -> Result<Response<LoadSessionResponse>, Status> {
        sessions::load_session(self.memory(), request).await
    }

    async fn search_sessions(
        &self,
        request: Request<SearchSessionsRequest>,
    ) -> Result<Response<SearchSessionsResponse>, Status> {
        sessions::search_sessions(self.memory(), request).await
    }

    async fn update_session_metadata(
        &self,
        request: Request<UpdateSessionMetadataRequest>,
    ) -> Result<Response<UpdateSessionMetadataResponse>, Status> {
        sessions::update_session_metadata(self.memory(), request).await
    }

    async fn delete_session(
        &self,
        request: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        sessions::delete_session(self.memory(), request).await
    }

    async fn delete_entity(
        &self,
        request: Request<DeleteEntityRequest>,
    ) -> Result<Response<DeleteEntityResponse>, Status> {
        records::delete_entity(self.memory(), request).await
    }

    async fn merge_entities(
        &self,
        request: Request<MergeEntitiesRequest>,
    ) -> Result<Response<MergeEntitiesResponse>, Status> {
        records::merge_entities(self.memory(), request).await
    }

    async fn forget_update(
        &self,
        request: Request<UpdateRefRequest>,
    ) -> Result<Response<UpdateRefResponse>, Status> {
        records::forget_update(self.memory(), request).await
    }

    async fn purge_update(
        &self,
        request: Request<UpdateRefRequest>,
    ) -> Result<Response<UpdateRefResponse>, Status> {
        records::purge_update(self.memory(), request).await
    }

    async fn get_graph_view(
        &self,
        request: Request<GetGraphViewRequest>,
    ) -> Result<Response<GraphView>, Status> {
        views::get_graph_view(self.memory(), request).await
    }

    async fn update_context(
        &self,
        request: Request<UpdateContextRequest>,
    ) -> Result<Response<UpdateContextResponse>, Status> {
        records::update_context(self.memory(), request).await
    }

    async fn bulk_update_context(
        &self,
        request: Request<BulkUpdateContextRequest>,
    ) -> Result<Response<BulkUpdateContextResponse>, Status> {
        records::bulk_update_context(self.memory(), request).await
    }

    async fn create_checkpoint(
        &self,
        request: Request<CreateCheckpointRequest>,
    ) -> Result<Response<CreateCheckpointResponse>, Status> {
        records::create_checkpoint(self.memory(), request).await
    }

    async fn query_context(
        &self,
        request: Request<QueryContextRequest>,
    ) -> Result<Response<QueryContextResponse>, Status> {
        records::query_context(self.memory(), request).await
    }

    async fn semantic_search(
        &self,
        request: Request<SemanticSearchRequest>,
    ) -> Result<Response<SemanticSearchResponse>, Status> {
        search::semantic_search(self.memory(), request).await
    }

    async fn find_related_content(
        &self,
        request: Request<FindRelatedContentRequest>,
    ) -> Result<Response<FindRelatedContentResponse>, Status> {
        search::find_related_content(self.memory(), request).await
    }

    async fn vectorize_session(
        &self,
        request: Request<VectorizeSessionRequest>,
    ) -> Result<Response<VectorizeSessionResponse>, Status> {
        search::vectorize_session(self.memory(), request).await
    }

    async fn get_vectorization_stats(
        &self,
        _request: Request<GetVectorizationStatsRequest>,
    ) -> Result<Response<VectorizationStats>, Status> {
        search::get_vectorization_stats(self.memory(), _request).await
    }

    async fn get_structured_summary(
        &self,
        request: Request<GetStructuredSummaryRequest>,
    ) -> Result<Response<StructuredSummary>, Status> {
        views::get_structured_summary(self.memory(), request).await
    }

    async fn get_key_decisions(
        &self,
        request: Request<GetKeyDecisionsRequest>,
    ) -> Result<Response<KeyDecisions>, Status> {
        views::get_key_decisions(self.memory(), request).await
    }

    async fn get_key_insights(
        &self,
        request: Request<GetKeyInsightsRequest>,
    ) -> Result<Response<KeyInsights>, Status> {
        views::get_key_insights(self.memory(), request).await
    }

    async fn get_entity_importance(
        &self,
        request: Request<GetEntityImportanceRequest>,
    ) -> Result<Response<EntityImportance>, Status> {
        views::get_entity_importance(self.memory(), request).await
    }

    async fn get_entity_network(
        &self,
        request: Request<GetEntityNetworkRequest>,
    ) -> Result<Response<EntityNetwork>, Status> {
        views::get_entity_network(self.memory(), request).await
    }

    async fn get_session_statistics(
        &self,
        request: Request<GetSessionStatisticsRequest>,
    ) -> Result<Response<SessionStatistics>, Status> {
        views::get_session_statistics(self.memory(), request).await
    }

    async fn assemble_context(
        &self,
        request: Request<AssembleContextRequest>,
    ) -> Result<Response<AssembleContextResponse>, Status> {
        assembly::assemble_context(self.memory(), request).await
    }

    async fn create_workspace(
        &self,
        request: Request<CreateWorkspaceRequest>,
    ) -> Result<Response<CreateWorkspaceResponse>, Status> {
        workspaces::create_workspace(self.memory(), request).await
    }

    async fn get_workspace(
        &self,
        request: Request<GetWorkspaceRequest>,
    ) -> Result<Response<GetWorkspaceResponse>, Status> {
        workspaces::get_workspace(self.memory(), request).await
    }

    async fn list_workspaces(
        &self,
        request: Request<ListWorkspacesRequest>,
    ) -> Result<Response<ListWorkspacesResponse>, Status> {
        workspaces::list_workspaces(self.memory(), request).await
    }

    async fn delete_workspace(
        &self,
        request: Request<DeleteWorkspaceRequest>,
    ) -> Result<Response<DeleteWorkspaceResponse>, Status> {
        workspaces::delete_workspace(self.memory(), request).await
    }

    async fn add_session_to_workspace(
        &self,
        request: Request<AddSessionToWorkspaceRequest>,
    ) -> Result<Response<AddSessionToWorkspaceResponse>, Status> {
        workspaces::add_session_to_workspace(self.memory(), request).await
    }

    async fn remove_session_from_workspace(
        &self,
        request: Request<RemoveSessionFromWorkspaceRequest>,
    ) -> Result<Response<RemoveSessionFromWorkspaceResponse>, Status> {
        workspaces::remove_session_from_workspace(self.memory(), request).await
    }

    async fn resolve_project(
        &self,
        request: Request<ResolveProjectRequest>,
    ) -> Result<Response<ResolveProjectResponse>, Status> {
        projects::resolve_project(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn find_project(
        &self,
        request: Request<FindProjectRequest>,
    ) -> Result<Response<FindProjectResponse>, Status> {
        Ok(Response::new(projects::find_project(
            self.memory(),
            request.into_inner(),
        )))
    }

    async fn update_workspace(
        &self,
        request: Request<UpdateWorkspaceRequest>,
    ) -> Result<Response<UpdateWorkspaceResponse>, Status> {
        lifecycle::update_workspace(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn archive_workspace(
        &self,
        request: Request<ArchiveWorkspaceRequest>,
    ) -> Result<Response<ArchiveWorkspaceResponse>, Status> {
        lifecycle::set_archived(self.memory(), request.into_inner(), true)
            .await
            .map(Response::new)
    }

    async fn unarchive_workspace(
        &self,
        request: Request<ArchiveWorkspaceRequest>,
    ) -> Result<Response<ArchiveWorkspaceResponse>, Status> {
        lifecycle::set_archived(self.memory(), request.into_inner(), false)
            .await
            .map(Response::new)
    }

    async fn delete_message(
        &self,
        request: Request<DeleteMessageRequest>,
    ) -> Result<Response<DeleteMessagesResponse>, Status> {
        lifecycle::delete_message(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn delete_messages(
        &self,
        request: Request<DeleteMessagesRequest>,
    ) -> Result<Response<DeleteMessagesResponse>, Status> {
        lifecycle::delete_messages(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn dismiss_radar_item(
        &self,
        request: Request<DismissRadarItemRequest>,
    ) -> Result<Response<DismissRadarItemResponse>, Status> {
        lifecycle::dismiss(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn restore_radar_item(
        &self,
        request: Request<RestoreRadarItemRequest>,
    ) -> Result<Response<RestoreRadarItemResponse>, Status> {
        lifecycle::restore(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn list_radar_dismissals(
        &self,
        request: Request<ListRadarDismissalsRequest>,
    ) -> Result<Response<ListRadarDismissalsResponse>, Status> {
        lifecycle::list_dismissals(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn export(
        &self,
        request: Request<ExportRequest>,
    ) -> Result<Response<Self::ExportStream>, Status> {
        crate::transfer::export(self.memory(), &request.into_inner())
            .await
            .map(Response::new)
    }

    async fn import(
        &self,
        request: Request<tonic::Streaming<ImportChunk>>,
    ) -> Result<Response<ImportResponse>, Status> {
        crate::transfer::import(self.memory(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn register_source(
        &self,
        request: Request<RegisterSourceRequest>,
    ) -> Result<Response<RegisterSourceAck>, Status> {
        freshness::register_source_impl(self.memory(), request).await
    }

    async fn register_source_batch(
        &self,
        request: Request<RegisterSourceBatchRequest>,
    ) -> Result<Response<RegisterSourceBatchAck>, Status> {
        freshness::register_source_batch_impl(self.memory(), request).await
    }

    async fn check_freshness(
        &self,
        request: Request<FreshnessRequest>,
    ) -> Result<Response<FreshnessReport>, Status> {
        freshness::check_freshness_impl(self.memory(), request).await
    }

    async fn invalidate(
        &self,
        request: Request<InvalidateRequest>,
    ) -> Result<Response<InvalidateAck>, Status> {
        freshness::invalidate_impl(self.memory(), request).await
    }

    async fn register_symbol_dependency(
        &self,
        request: Request<RegisterSymbolDependencyRequest>,
    ) -> Result<Response<RegisterSymbolDependencyAck>, Status> {
        freshness::register_symbol_dependency_impl(self.memory(), request).await
    }

    async fn cascade_invalidate(
        &self,
        request: Request<CascadeInvalidateRequest>,
    ) -> Result<Response<CascadeInvalidateReport>, Status> {
        freshness::cascade_invalidate_impl(self.memory(), request).await
    }

    async fn get_stale_entries_by_source(
        &self,
        request: Request<GetStaleEntriesBySourceRequest>,
    ) -> Result<Response<GetStaleEntriesBySourceResponse>, Status> {
        freshness::get_stale_entries_by_source_impl(self.memory(), request).await
    }
}
