// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! gRPC service for Post-Cortex.
//!
//! Provides a tonic gRPC interface to the memory system, enabling
//! native binary protocol access for coding agents like Axon.
//!
//! Layout: the canonical `impl PostCortex for PcxGrpcService` block lives in
//! this file. Freshness-tracking methods (Phase 9) delegate to inherent
//! `*_impl` methods in `freshness` to keep this file scoped to the
//! interactive surface. Shared parsing/validation helpers live in
//! `helpers`.

use post_cortex_core::services::PostCortexService;
use post_cortex_memory::services::MemoryServiceImpl;
use post_cortex_memory::ConversationMemorySystem;
use post_cortex_storage::rocksdb_storage::SessionCheckpoint;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Phase 2 of the workspace refactor moved the proto-generated bindings into
// the dedicated `post-cortex-proto` crate. The re-export below preserves the
// long-standing `crate::daemon::grpc_service::pb::…` path for in-tree
// callers inside `src/daemon/` while every consumer in `src/core/` and
// `src/storage/` now imports from `post_cortex_proto::pb` directly.
pub use post_cortex_proto::pb;

mod freshness;
mod helpers;

use helpers::{
    parse_session_role, parse_uuid, proto_bulk_item_to_request, proto_update_to_request,
    system_error_to_status, workspace_to_info,
};
use pb::post_cortex_server::{PostCortex, PostCortexServer};
use pb::*;

/// gRPC service backed by ConversationMemorySystem.
///
/// Holds both the raw [`ConversationMemorySystem`] (used by handlers
/// that have not yet migrated to the canonical trait) and the
/// canonical [`MemoryServiceImpl`]. Newly-migrated handlers — currently
/// `update_context` / `bulk_update_context` — translate proto types
/// into [`post_cortex_core::services`] requests and delegate to
/// `self.service`, so validation lives in exactly one place.
pub struct PcxGrpcService {
    pub(super) memory: Arc<ConversationMemorySystem>,
    pub(super) service: Arc<MemoryServiceImpl>,
}

impl PcxGrpcService {
    /// Wrap a shared memory system in a new gRPC service handle.
    pub fn new(memory: Arc<ConversationMemorySystem>) -> Self {
        let service = Arc::new(MemoryServiceImpl::new(memory.clone()));
        Self { memory, service }
    }

    /// Build a gRPC service from an existing canonical service so several
    /// transports can share the same background pipeline (Phase 11).
    pub fn from_service(service: Arc<MemoryServiceImpl>) -> Self {
        let memory = service.inner().clone();
        Self { memory, service }
    }

    /// Consume `self` and return a tonic [`PostCortexServer`] ready to serve.
    pub fn into_server(self) -> PostCortexServer<Self> {
        PostCortexServer::new(self)
    }
}

#[tonic::async_trait]
impl PostCortex for PcxGrpcService {
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let health = self.memory.get_system_health();

        Ok(Response::new(HealthResponse {
            healthy: !health.circuit_breaker_open,
            version: env!("CARGO_PKG_VERSION").to_string(),
            active_sessions: health.active_sessions as u64,
            total_updates: health.total_requests,
            embeddings_enabled: self.memory.embeddings_enabled(),
        }))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC CreateSession: name={}", req.name);

        let name = if req.name.is_empty() {
            None
        } else {
            Some(req.name)
        };
        let description = if req.description.is_empty() {
            None
        } else {
            Some(req.description)
        };

        match self.memory.create_session(name, description).await {
            Ok(session_id) => Ok(Response::new(CreateSessionResponse {
                session_id: session_id.to_string(),
            })),
            Err(e) => {
                error!("gRPC CreateSession failed: {}", e);
                Err(Status::internal(e))
            }
        }
    }

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let req = request.into_inner();

        let session_ids = if !req.name_filter.is_empty() {
            self.memory
                .find_sessions_by_name_or_description(&req.name_filter)
                .await
                .map_err(Status::internal)?
        } else {
            self.memory
                .list_sessions()
                .await
                .map_err(Status::internal)?
        };

        let limit = if req.limit > 0 {
            req.limit as usize
        } else {
            100
        };

        let mut sessions = Vec::new();
        for session_id in session_ids.into_iter().take(limit) {
            if let Ok(session_arc) = self.memory.get_session(session_id).await {
                let session = session_arc.load();
                sessions.push(SessionInfo {
                    session_id: session_id.to_string(),
                    name: session.name().unwrap_or_default(),
                    description: session.description().unwrap_or_default(),
                    created_at_unix: session.created_at().timestamp(),
                    update_count: session.incremental_updates.len() as u32,
                });
            }
        }

        Ok(Response::new(ListSessionsResponse { sessions }))
    }

    async fn load_session(
        &self,
        request: Request<LoadSessionRequest>,
    ) -> Result<Response<LoadSessionResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC LoadSession: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;

        let session_arc = self
            .memory
            .get_session(session_id)
            .await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        let info = SessionInfo {
            session_id: session_id.to_string(),
            name: session.name().unwrap_or_default(),
            description: session.description().unwrap_or_default(),
            created_at_unix: session.created_at().timestamp(),
            update_count: session.incremental_updates.len() as u32,
        };

        Ok(Response::new(LoadSessionResponse {
            session: Some(info),
        }))
    }

    async fn search_sessions(
        &self,
        request: Request<SearchSessionsRequest>,
    ) -> Result<Response<SearchSessionsResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC SearchSessions: query={}", req.query);

        if req.query.is_empty() {
            return Err(Status::invalid_argument("query cannot be empty"));
        }

        let session_ids = self
            .memory
            .find_sessions_by_name_or_description(&req.query)
            .await
            .map_err(Status::internal)?;

        let mut sessions = Vec::new();
        for session_id in session_ids {
            if let Ok(session_arc) = self.memory.get_session(session_id).await {
                let session = session_arc.load();
                sessions.push(SessionInfo {
                    session_id: session_id.to_string(),
                    name: session.name().unwrap_or_default(),
                    description: session.description().unwrap_or_default(),
                    created_at_unix: session.created_at().timestamp(),
                    update_count: session.incremental_updates.len() as u32,
                });
            }
        }

        Ok(Response::new(SearchSessionsResponse { sessions }))
    }

    async fn update_session_metadata(
        &self,
        request: Request<UpdateSessionMetadataRequest>,
    ) -> Result<Response<UpdateSessionMetadataResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC UpdateSessionMetadata: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;

        match self
            .memory
            .update_session_metadata(session_id, req.name, req.description)
            .await
        {
            Ok(()) => Ok(Response::new(UpdateSessionMetadataResponse {
                success: true,
            })),
            Err(e) => {
                error!("gRPC UpdateSessionMetadata failed: {}", e);
                Err(Status::internal(e))
            }
        }
    }

    async fn delete_session(
        &self,
        request: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC DeleteSession: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;

        match self.memory.delete_session(session_id).await {
            Ok(success) => Ok(Response::new(DeleteSessionResponse { success })),
            Err(e) => {
                error!("gRPC DeleteSession failed: {}", e);
                Err(Status::internal(e))
            }
        }
    }

    async fn delete_entity(
        &self,
        request: Request<DeleteEntityRequest>,
    ) -> Result<Response<DeleteEntityResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC DeleteEntity: session_id={} entity={}",
            req.session_id, req.entity_name
        );

        if req.entity_name.is_empty() {
            return Err(Status::invalid_argument("entity_name must not be empty"));
        }
        let session_id = parse_uuid(&req.session_id)?;

        match self.memory.delete_entity(session_id, &req.entity_name).await {
            Ok(existed) => Ok(Response::new(DeleteEntityResponse { existed })),
            Err(e) => {
                error!("gRPC DeleteEntity failed: {}", e);
                Err(Status::internal(e))
            }
        }
    }

    async fn update_context(
        &self,
        request: Request<UpdateContextRequest>,
    ) -> Result<Response<UpdateContextResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC UpdateContext: session={}, type={}",
            req.session_id, req.interaction_type
        );

        let service_req = proto_update_to_request(req)?;
        let resp = self
            .service
            .update_context(service_req)
            .await
            .map_err(|e| {
                error!("gRPC UpdateContext failed: {}", e);
                system_error_to_status(e)
            })?;

        Ok(Response::new(UpdateContextResponse {
            update_id: resp.entry_id.to_string(),
            success: resp.durable,
        }))
    }

    async fn bulk_update_context(
        &self,
        request: Request<BulkUpdateContextRequest>,
    ) -> Result<Response<BulkUpdateContextResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC BulkUpdateContext: session={}, count={}",
            req.session_id,
            req.updates.len()
        );

        // Translate every proto item to its canonical request, propagating
        // session_id from the batch envelope (proto items don't carry it).
        let batch_session_id = parse_uuid(&req.session_id)?;
        let mut update_ids = Vec::new();
        let mut success_count = 0u32;
        let mut failure_count = 0u32;

        for item in req.updates {
            let service_req = match proto_bulk_item_to_request(batch_session_id, item) {
                Ok(r) => r,
                Err(status) => {
                    warn!(
                        "gRPC BulkUpdateContext item translation failed: {}",
                        status.message()
                    );
                    failure_count += 1;
                    continue;
                }
            };

            match self.service.update_context(service_req).await {
                Ok(resp) => {
                    update_ids.push(resp.entry_id.to_string());
                    success_count += 1;
                }
                Err(e) => {
                    error!("gRPC BulkUpdateContext item failed: {}", e);
                    failure_count += 1;
                }
            }
        }

        Ok(Response::new(BulkUpdateContextResponse {
            update_ids,
            success_count,
            failure_count,
        }))
    }

    async fn create_checkpoint(
        &self,
        request: Request<CreateCheckpointRequest>,
    ) -> Result<Response<CreateCheckpointResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC CreateCheckpoint: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;

        let session_arc = self
            .memory
            .get_session(session_id)
            .await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        let checkpoint = SessionCheckpoint {
            id: Uuid::new_v4(),
            session_id,
            created_at: chrono::Utc::now(),
            structured_context: (*session.current_state).clone(),
            recent_updates: (*session.incremental_updates).clone(),
            code_references: (*session.code_references).clone(),
            change_history: (*session.change_history).clone(),
            total_updates: session.incremental_updates.len(),
            context_quality_score: 1.0,
            compression_ratio: 1.0,
        };
        let checkpoint_id = checkpoint.id.to_string();

        match self.memory.storage_actor.save_checkpoint(&checkpoint).await {
            Ok(()) => Ok(Response::new(CreateCheckpointResponse {
                checkpoint_id,
                success: true,
            })),
            Err(e) => {
                error!("gRPC CreateCheckpoint failed: {}", e);
                Err(Status::internal(e))
            }
        }
    }

    async fn query_context(
        &self,
        request: Request<QueryContextRequest>,
    ) -> Result<Response<QueryContextResponse>, Status> {
        let req = request.into_inner();
        let session_id = parse_uuid(&req.session_id)?;

        let session_arc = self
            .memory
            .get_session(session_id)
            .await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        let limit = if req.limit > 0 {
            req.limit as usize
        } else {
            50
        };

        let updates: Vec<ContextUpdateEntry> = session
            .incremental_updates
            .iter()
            .filter(|u| {
                if !req.interaction_type.is_empty() {
                    let ut = format!("{:?}", u.update_type);
                    ut.to_lowercase()
                        .contains(&req.interaction_type.to_lowercase())
                } else {
                    true
                }
            })
            .filter(|u| {
                if req.after_unix > 0 {
                    u.timestamp.timestamp() > req.after_unix
                } else {
                    true
                }
            })
            .take(limit)
            .map(|u| ContextUpdateEntry {
                id: u.id.to_string(),
                interaction_type: format!("{:?}", u.update_type),
                content: Some(ContextContent {
                    title: u.content.title.clone(),
                    description: u.content.description.clone(),
                    details: u.content.details.clone(),
                    examples: u.content.examples.clone(),
                    implications: u.content.implications.clone(),
                    code_ref: u.related_code.as_ref().map(|c| CodeReference {
                        file_path: c.file_path.clone(),
                        start_line: c.start_line,
                        end_line: c.end_line,
                        code_snippet: c.code_snippet.clone(),
                        commit_hash: c.commit_hash.clone().unwrap_or_default(),
                        branch: c.branch.clone().unwrap_or_default(),
                        change_description: c.change_description.clone(),
                    }),
                }),
                timestamp_unix: u.timestamp.timestamp(),
                entities: u.creates_entities.clone(),
                source_ref: None, // Source tracking not yet wired
            })
            .collect();

        let total = updates.len() as u32;
        Ok(Response::new(QueryContextResponse { updates, total }))
    }

    async fn semantic_search(
        &self,
        request: Request<SemanticSearchRequest>,
    ) -> Result<Response<SemanticSearchResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC SemanticSearch: query='{}', session='{}', workspace='{:?}'",
            req.query, req.session_id, req.workspace_id
        );

        if req.query.is_empty() {
            return Err(Status::invalid_argument("query cannot be empty"));
        }

        let max_results = if req.max_results > 0 {
            req.max_results as usize
        } else {
            10
        };

        #[cfg(feature = "embeddings")]
        {
            // workspace_id takes precedence: search each session independently and merge results
            let workspace_id_str = req.workspace_id.as_deref().unwrap_or("");
            let search_results = if !workspace_id_str.is_empty() {
                // Workspace-scoped: search all sessions in the workspace and merge
                let workspace_id = parse_uuid(workspace_id_str)?;
                let workspace = self
                    .memory
                    .workspace_manager
                    .get_workspace(&workspace_id)
                    .ok_or_else(|| Status::not_found(format!("Workspace {workspace_id} not found")))?;

                let session_ids: Vec<uuid::Uuid> = workspace
                    .get_all_sessions()
                    .into_iter()
                    .map(|(sid, _)| sid)
                    .collect();

                let mut merged = Vec::new();
                for sid in session_ids {
                    match self
                        .memory
                        .semantic_search_session(sid, &req.query, Some(max_results), None, None)
                        .await
                    {
                        Ok(results) => merged.extend(results),
                        Err(e) => warn!(
                            "SemanticSearch: session {} search failed: {}",
                            sid, e
                        ),
                    }
                }
                // Sort by combined_score descending and cap at max_results
                merged.sort_by(|a, b| b.combined_score.partial_cmp(&a.combined_score).unwrap_or(std::cmp::Ordering::Equal));
                merged.truncate(max_results);
                merged
            } else if req.session_id.is_empty() {
                // Global search
                self.memory
                    .semantic_search_global(&req.query, Some(max_results), None, None)
                    .await
                    .map_err(|e| Status::internal(format!("Search failed: {e}")))?
            } else {
                // Session-scoped search
                let session_id = parse_uuid(&req.session_id)?;
                self.memory
                    .semantic_search_session(session_id, &req.query, Some(max_results), None, None)
                    .await
                    .map_err(|e| Status::internal(format!("Search failed: {e}")))?
            };

            let min_score = if req.min_score > 0.0 {
                req.min_score
            } else {
                0.0
            };

            let results: Vec<SearchResult> = search_results
                .into_iter()
                .filter(|r| r.combined_score >= min_score)
                .map(|r| SearchResult {
                    entry_id: r.content_id,
                    content: r.text_content,
                    score: r.combined_score,
                    session_id: r.session_id.to_string(),
                    content_type: format!("{:?}", r.content_type),
                    metadata: std::collections::HashMap::new(),
                })
                .collect();

            let total_matches = results.len() as u32;
            Ok(Response::new(SemanticSearchResponse {
                results,
                total_matches,
            }))
        }

        #[cfg(not(feature = "embeddings"))]
        {
            Err(Status::unimplemented(
                "Semantic search requires the 'embeddings' feature",
            ))
        }
    }

    async fn find_related_content(
        &self,
        request: Request<FindRelatedContentRequest>,
    ) -> Result<Response<FindRelatedContentResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC FindRelatedContent: session={}, topic='{}'",
            req.session_id, req.topic
        );

        if req.topic.is_empty() {
            return Err(Status::invalid_argument("topic cannot be empty"));
        }

        #[cfg(feature = "embeddings")]
        {
            let session_id = parse_uuid(&req.session_id)?;
            let max_results = req.limit.unwrap_or(10) as usize;

            let search_results = self
                .memory
                .semantic_search_session(session_id, &req.topic, Some(max_results), None, None)
                .await
                .map_err(|e| Status::internal(format!("Search failed: {e}")))?;

            let results: Vec<SearchResult> = search_results
                .into_iter()
                .map(|r| SearchResult {
                    entry_id: r.content_id,
                    content: r.text_content,
                    score: r.combined_score,
                    session_id: r.session_id.to_string(),
                    content_type: format!("{:?}", r.content_type),
                    metadata: std::collections::HashMap::new(),
                })
                .collect();

            Ok(Response::new(FindRelatedContentResponse { results }))
        }

        #[cfg(not(feature = "embeddings"))]
        {
            Err(Status::unimplemented(
                "FindRelatedContent requires the 'embeddings' feature",
            ))
        }
    }

    async fn vectorize_session(
        &self,
        request: Request<VectorizeSessionRequest>,
    ) -> Result<Response<VectorizeSessionResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC VectorizeSession: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;

        #[cfg(feature = "embeddings")]
        {
            match self.memory.vectorize_session(session_id).await {
                Ok(vectors_created) => Ok(Response::new(VectorizeSessionResponse {
                    success: true,
                    vectors_created: vectors_created as u32,
                })),
                Err(e) => {
                    error!("gRPC VectorizeSession failed: {}", e);
                    Err(Status::internal(e))
                }
            }
        }

        #[cfg(not(feature = "embeddings"))]
        {
            Err(Status::unimplemented(
                "VectorizeSession requires the 'embeddings' feature",
            ))
        }
    }

    async fn get_vectorization_stats(
        &self,
        _request: Request<GetVectorizationStatsRequest>,
    ) -> Result<Response<TextResponse>, Status> {
        debug!("gRPC GetVectorizationStats");

        #[cfg(feature = "embeddings")]
        {
            match self.memory.get_vectorization_stats() {
                Ok(stats) => {
                    let text =
                        serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string());
                    Ok(Response::new(TextResponse { text }))
                }
                Err(e) => {
                    error!("gRPC GetVectorizationStats failed: {}", e);
                    Err(Status::internal(e))
                }
            }
        }

        #[cfg(not(feature = "embeddings"))]
        {
            Err(Status::unimplemented(
                "GetVectorizationStats requires the 'embeddings' feature",
            ))
        }
    }

    // --- Analysis & Insights ---

    async fn get_structured_summary(
        &self,
        request: Request<GetStructuredSummaryRequest>,
    ) -> Result<Response<TextResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetStructuredSummary: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;
        let session_arc = self.memory.get_session(session_id).await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        use post_cortex_core::summary::{SummaryGenerator, SummaryOptions};
        let user_requested_compact = req.compact.unwrap_or(false);
        const MAX_TOKENS: usize = 50_000;

        let (_estimated_tokens, should_compact) =
            SummaryGenerator::estimate_summary_size(&session, MAX_TOKENS);
        let auto_compacted = !user_requested_compact && should_compact;

        let options = if user_requested_compact || auto_compacted {
            SummaryOptions::compact()
        } else {
            SummaryOptions {
                decisions_limit: req.decisions_limit.map(|v| v as usize),
                entities_limit: req.entities_limit.map(|v| v as usize),
                questions_limit: req.questions_limit.map(|v| v as usize),
                concepts_limit: req.concepts_limit.map(|v| v as usize),
                min_confidence: req.min_confidence,
                compact: false,
            }
        };

        let summary = SummaryGenerator::generate_structured_summary_filtered(&session, &options);
        let text = serde_json::to_string_pretty(&summary)
            .unwrap_or_else(|_| format!("{summary:?}"));

        Ok(Response::new(TextResponse { text }))
    }

    async fn get_key_decisions(
        &self,
        request: Request<GetKeyDecisionsRequest>,
    ) -> Result<Response<TextResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetKeyDecisions: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;
        let session_arc = self.memory.get_session(session_id).await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        use post_cortex_core::summary::SummaryGenerator;
        let decisions = SummaryGenerator::extract_decision_timeline(&session);
        let text = serde_json::to_string_pretty(&decisions)
            .unwrap_or_else(|_| format!("{decisions:?}"));

        Ok(Response::new(TextResponse { text }))
    }

    async fn get_key_insights(
        &self,
        request: Request<GetKeyInsightsRequest>,
    ) -> Result<Response<TextResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetKeyInsights: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;
        let session_arc = self.memory.get_session(session_id).await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        use post_cortex_core::summary::SummaryGenerator;
        let insights = SummaryGenerator::extract_key_insights(&session, req.limit.map(|v| v as usize).unwrap_or(5));
        let text = serde_json::to_string_pretty(&insights)
            .unwrap_or_else(|_| format!("{insights:?}"));

        Ok(Response::new(TextResponse { text }))
    }

    async fn get_entity_importance(
        &self,
        request: Request<GetEntityImportanceRequest>,
    ) -> Result<Response<TextResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetEntityImportance: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;
        let session_arc = self.memory.get_session(session_id).await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        let mut analysis = session.entity_graph.analyze_entity_importance();
        if let Some(min_imp) = req.min_importance {
            analysis.retain(|a| a.importance_score >= min_imp);
        }
        if let Some(limit) = req.limit {
            analysis.truncate(limit as usize);
        }
        let text = serde_json::to_string_pretty(&analysis)
            .unwrap_or_else(|_| format!("{analysis:?}"));

        Ok(Response::new(TextResponse { text }))
    }

    async fn get_entity_network(
        &self,
        request: Request<GetEntityNetworkRequest>,
    ) -> Result<Response<TextResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetEntityNetwork: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;
        let session_arc = self.memory.get_session(session_id).await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        let network = match req.center_entity {
            Some(entity) => session.entity_graph.get_entity_network(&entity, 2),
            None => {
                let top_entities = session.entity_graph.get_most_important_entities(1);
                if let Some(top) = top_entities.first() {
                    session.entity_graph.get_entity_network(&top.name, 2)
                } else {
                    session.entity_graph.get_entity_network("", 2)
                }
            }
        };
        let text = serde_json::to_string_pretty(&network)
            .unwrap_or_else(|_| format!("{network:?}"));

        Ok(Response::new(TextResponse { text }))
    }

    async fn get_session_statistics(
        &self,
        request: Request<GetSessionStatisticsRequest>,
    ) -> Result<Response<TextResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetSessionStatistics: session_id={}", req.session_id);

        let session_id = parse_uuid(&req.session_id)?;
        let session_arc = self.memory.get_session(session_id).await
            .map_err(Status::not_found)?;
        let session = session_arc.load();

        use post_cortex_core::summary::SummaryGenerator;
        let stats = SummaryGenerator::calculate_session_stats(&session);
        let text = serde_json::to_string_pretty(&stats)
            .unwrap_or_else(|_| format!("{stats:?}"));

        Ok(Response::new(TextResponse { text }))
    }

    // --- Graph-Aware Context Assembly ---

    async fn assemble_context(
        &self,
        request: Request<AssembleContextRequest>,
    ) -> Result<Response<AssembleContextResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC AssembleContext: session_id={}, workspace_id={:?}, query={}",
            req.session_id, req.workspace_id, req.query
        );

        let token_budget = if req.token_budget == 0 { 4000 } else { req.token_budget as usize };

        use post_cortex_memory::context_assembly;
        use post_cortex_core::graph::entity_graph::SimpleEntityGraph;

        // Resolve scope: workspace_id takes precedence over session_id
        let (updates, graph) = if req.workspace_id.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            // Workspace-scoped: merge context from all sessions in the workspace
            let workspace_id = parse_uuid(req.workspace_id.as_deref().unwrap_or(""))?;
            let workspace = self
                .memory
                .workspace_manager
                .get_workspace(&workspace_id)
                .ok_or_else(|| Status::not_found(format!("Workspace {workspace_id} not found")))?;

            let mut all_updates = Vec::new();
            let mut merged_graph = SimpleEntityGraph::new();

            for (ws_session_id, _role) in workspace.get_all_sessions() {
                match self.memory.get_session(ws_session_id).await {
                    Ok(session_arc) => {
                        let session = session_arc.load();
                        all_updates.extend(
                            session.hot_context.iter().iter().cloned(),
                        );
                        all_updates.extend(
                            session.warm_context.iter().map(|c| c.update.clone()),
                        );
                        merged_graph.merge_from(&session.entity_graph);
                    }
                    Err(e) => {
                        warn!(
                            "AssembleContext: could not load session {} from workspace {}: {}",
                            ws_session_id, workspace_id, e
                        );
                    }
                }
            }

            (all_updates, merged_graph)
        } else {
            // Single-session (existing behaviour)
            let session_id = parse_uuid(&req.session_id)?;
            let session_arc = self.memory.get_session(session_id).await
                .map_err(Status::not_found)?;
            let session = session_arc.load();
            let updates: Vec<_> = session.hot_context.iter().iter()
                .chain(session.warm_context.iter().map(|c| &c.update))
                .cloned()
                .collect();
            (updates, (*session.entity_graph).clone())
        };

        let assembled = context_assembly::assemble_context(
            &req.query,
            &graph,
            &updates,
            token_budget,
        );

        let formatted_text = context_assembly::format_for_llm(&assembled);

        // Convert to proto types
        let items: Vec<AssembledContextItem> = assembled.items.iter().map(|item| {
            let (source, via_entity) = match &item.source {
                context_assembly::ContextSource::SemanticMatch => ("semantic_match".to_string(), String::new()),
                context_assembly::ContextSource::GraphTraversal { via_entity } => ("graph_traversal".to_string(), via_entity.clone()),
                context_assembly::ContextSource::RecentUpdate => ("recent_update".to_string(), String::new()),
            };
            AssembledContextItem {
                text: item.text.clone(),
                score: item.score,
                source,
                via_entity,
                entities: item.entities.clone(),
                token_estimate: item.token_estimate as u32,
                entry_id: item.entry_id.clone(),
            }
        }).collect();

        let entity_context: Vec<AssembledEntityContext> = assembled.entity_context.iter().map(|ec| {
            let (relevance, via_entity, via_relation) = match &ec.relevance {
                context_assembly::EntityRelevance::DirectMention => ("direct_mention".to_string(), String::new(), String::new()),
                context_assembly::EntityRelevance::GraphNeighbor { via, relation } => ("graph_neighbor".to_string(), via.clone(), relation.clone()),
            };
            let relationships: Vec<EntityRelation> = ec.relationships.iter().map(|r| {
                EntityRelation {
                    from_entity: r.from_entity.clone(),
                    to_entity: r.to_entity.clone(),
                    relation_type: format!("{:?}", r.relation_type),
                    context: r.context.clone(),
                }
            }).collect();
            AssembledEntityContext {
                name: ec.name.clone(),
                relevance,
                via_entity,
                via_relation,
                relationships,
            }
        }).collect();

        let impact: Vec<pb::ImpactEntry> = assembled.impact.iter().map(|i| {
            pb::ImpactEntry {
                entity: i.entity.clone(),
                depends_on: i.depends_on.clone(),
                relation_type: format!("{:?}", i.relation_type),
                context: i.context.clone(),
            }
        }).collect();

        Ok(Response::new(AssembleContextResponse {
            items,
            entity_context,
            impact,
            total_tokens: assembled.total_tokens as u32,
            formatted_text,
        }))
    }

    // --- Workspace Management ---

    async fn create_workspace(
        &self,
        request: Request<CreateWorkspaceRequest>,
    ) -> Result<Response<CreateWorkspaceResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC CreateWorkspace: name={}", req.name);

        let workspace_id = self
            .memory
            .workspace_manager
            .create_workspace(req.name.clone(), req.description.clone());

        // Persist to storage
        if let Err(e) = self
            .memory
            .save_workspace_metadata(workspace_id, &req.name, &req.description, &[])
            .await
        {
            warn!("gRPC CreateWorkspace: failed to persist metadata: {}", e);
        }

        Ok(Response::new(CreateWorkspaceResponse {
            workspace_id: workspace_id.to_string(),
        }))
    }

    async fn get_workspace(
        &self,
        request: Request<GetWorkspaceRequest>,
    ) -> Result<Response<GetWorkspaceResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC GetWorkspace: workspace_id={}", req.workspace_id);

        let workspace_id = parse_uuid(&req.workspace_id)?;

        let workspace = self
            .memory
            .workspace_manager
            .get_workspace(&workspace_id)
            .ok_or_else(|| Status::not_found(format!("Workspace {} not found", workspace_id)))?;

        let info = workspace_to_info(&workspace);

        Ok(Response::new(GetWorkspaceResponse {
            workspace: Some(info),
        }))
    }

    async fn list_workspaces(
        &self,
        _request: Request<ListWorkspacesRequest>,
    ) -> Result<Response<ListWorkspacesResponse>, Status> {
        debug!("gRPC ListWorkspaces");

        let workspaces = self.memory.workspace_manager.list_workspaces();

        let infos: Vec<WorkspaceInfo> = workspaces.iter().map(|ws| workspace_to_info(ws)).collect();

        Ok(Response::new(ListWorkspacesResponse { workspaces: infos }))
    }

    async fn delete_workspace(
        &self,
        request: Request<DeleteWorkspaceRequest>,
    ) -> Result<Response<DeleteWorkspaceResponse>, Status> {
        let req = request.into_inner();
        debug!("gRPC DeleteWorkspace: workspace_id={}", req.workspace_id);

        let workspace_id = parse_uuid(&req.workspace_id)?;

        let deleted = self
            .memory
            .workspace_manager
            .delete_workspace(&workspace_id);

        let success = deleted.is_some();
        Ok(Response::new(DeleteWorkspaceResponse { success }))
    }

    async fn add_session_to_workspace(
        &self,
        request: Request<AddSessionToWorkspaceRequest>,
    ) -> Result<Response<AddSessionToWorkspaceResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC AddSessionToWorkspace: workspace={}, session={}, role={}",
            req.workspace_id, req.session_id, req.role
        );

        let workspace_id = parse_uuid(&req.workspace_id)?;
        let session_id = parse_uuid(&req.session_id)?;
        let role = parse_session_role(&req.role);

        self.memory
            .workspace_manager
            .add_session_to_workspace(&workspace_id, session_id, role)
            .map_err(Status::not_found)?;

        // Persist updated workspace membership
        if let Some(ws) = self.memory.workspace_manager.get_workspace(&workspace_id) {
            let session_ids: Vec<Uuid> = ws.session_ids.iter().map(|entry| *entry.key()).collect();
            if let Err(e) = self
                .memory
                .save_workspace_metadata(workspace_id, &ws.name, &ws.description, &session_ids)
                .await
            {
                warn!(
                    "gRPC AddSessionToWorkspace: failed to persist metadata: {}",
                    e
                );
            }
        }

        Ok(Response::new(AddSessionToWorkspaceResponse {
            success: true,
        }))
    }

    async fn remove_session_from_workspace(
        &self,
        request: Request<RemoveSessionFromWorkspaceRequest>,
    ) -> Result<Response<RemoveSessionFromWorkspaceResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC RemoveSessionFromWorkspace: workspace={}, session={}",
            req.workspace_id, req.session_id
        );

        let workspace_id = parse_uuid(&req.workspace_id)?;
        let session_id = parse_uuid(&req.session_id)?;

        self.memory
            .workspace_manager
            .remove_session_from_workspace(&workspace_id, &session_id)
            .map_err(Status::not_found)?;

        // Persist updated workspace membership
        if let Some(ws) = self.memory.workspace_manager.get_workspace(&workspace_id) {
            let session_ids: Vec<Uuid> = ws.session_ids.iter().map(|entry| *entry.key()).collect();
            if let Err(e) = self
                .memory
                .save_workspace_metadata(workspace_id, &ws.name, &ws.description, &session_ids)
                .await
            {
                warn!(
                    "gRPC RemoveSessionFromWorkspace: failed to persist metadata: {}",
                    e
                );
            }
        }

        Ok(Response::new(RemoveSessionFromWorkspaceResponse {
            success: true,
        }))
    }


    // --- Source Tracking (Phase 9): delegated to ./freshness.rs ---

    async fn register_source(
        &self,
        request: Request<RegisterSourceRequest>,
    ) -> Result<Response<RegisterSourceAck>, Status> {
        self.register_source_impl(request).await
    }

    async fn register_source_batch(
        &self,
        request: Request<RegisterSourceBatchRequest>,
    ) -> Result<Response<RegisterSourceBatchAck>, Status> {
        self.register_source_batch_impl(request).await
    }

    async fn check_freshness(
        &self,
        request: Request<FreshnessRequest>,
    ) -> Result<Response<FreshnessReport>, Status> {
        self.check_freshness_impl(request).await
    }

    async fn invalidate(
        &self,
        request: Request<InvalidateRequest>,
    ) -> Result<Response<InvalidateAck>, Status> {
        self.invalidate_impl(request).await
    }

    async fn register_symbol_dependency(
        &self,
        request: Request<RegisterSymbolDependencyRequest>,
    ) -> Result<Response<RegisterSymbolDependencyAck>, Status> {
        self.register_symbol_dependency_impl(request).await
    }

    async fn cascade_invalidate(
        &self,
        request: Request<CascadeInvalidateRequest>,
    ) -> Result<Response<CascadeInvalidateReport>, Status> {
        self.cascade_invalidate_impl(request).await
    }

    async fn get_stale_entries_by_source(
        &self,
        request: Request<GetStaleEntriesBySourceRequest>,
    ) -> Result<Response<GetStaleEntriesBySourceResponse>, Status> {
        self.get_stale_entries_by_source_impl(request).await
    }
}

/// Start the gRPC server on the given port.
/// Returns a future that runs until cancelled.
pub async fn start_grpc_server(
    memory: Arc<ConversationMemorySystem>,
    addr: std::net::SocketAddr,
) -> Result<(), String> {
    let service = PcxGrpcService::new(memory);

    info!("Starting gRPC server on {}", addr);

    tonic::transport::Server::builder()
        .add_service(service.into_server())
        .serve(addr)
        .await
        .map_err(|e| format!("gRPC server error: {e}"))
}
