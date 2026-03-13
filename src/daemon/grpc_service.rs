//! gRPC service for Post-Cortex
//!
//! Provides a tonic gRPC interface to ConversationMemorySystem,
//! enabling native binary protocol access for coding agents like Axon.

use crate::ConversationMemorySystem;
use crate::storage::rocksdb_storage::SessionCheckpoint;
use crate::workspace::SessionRole;
use futures::future::join_all;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

pub mod pb {
    tonic::include_proto!("pcx.v1");
}

use pb::post_cortex_server::{PostCortex, PostCortexServer};
use pb::*;

/// gRPC service backed by ConversationMemorySystem
pub struct PcxGrpcService {
    memory: Arc<ConversationMemorySystem>,
}

impl PcxGrpcService {
    pub fn new(memory: Arc<ConversationMemorySystem>) -> Self {
        Self { memory }
    }

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
                .map_err(|e| Status::internal(e))?
        } else {
            self.memory
                .list_sessions()
                .await
                .map_err(|e| Status::internal(e))?
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
            .map_err(|e| Status::not_found(e))?;
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
            .map_err(|e| Status::internal(e))?;

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

        match self.memory.storage_actor.delete_session(session_id).await {
            Ok(success) => Ok(Response::new(DeleteSessionResponse { success })),
            Err(e) => {
                error!("gRPC DeleteSession failed: {}", e);
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

        let session_id = parse_uuid(&req.session_id)?;

        // Build the description from ContextContent
        let content = req.content.unwrap_or_default();
        let description = format_context_description(&req.interaction_type, &content);

        // Build metadata JSON matching what MCP tools produce
        let metadata = build_update_metadata(&req.interaction_type, &content);

        match self
            .memory
            .add_incremental_update(session_id, description, Some(metadata))
            .await
        {
            Ok(update_id) => Ok(Response::new(UpdateContextResponse {
                update_id,
                success: true,
            })),
            Err(e) => {
                error!("gRPC UpdateContext failed: {}", e);
                Err(Status::internal(e))
            }
        }
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

        let session_id = parse_uuid(&req.session_id)?;

        let mut update_ids = Vec::new();
        let mut success_count = 0u32;
        let mut failure_count = 0u32;

        for item in req.updates {
            let content = item.content.unwrap_or_default();
            let description = format_context_description(&item.interaction_type, &content);
            let metadata = build_update_metadata(&item.interaction_type, &content);

            match self
                .memory
                .add_incremental_update(session_id, description, Some(metadata))
                .await
            {
                Ok(update_id) => {
                    update_ids.push(update_id);
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
            .map_err(|e| Status::not_found(e))?;
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
            .map_err(|e| Status::not_found(e))?;
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
            "gRPC SemanticSearch: query='{}', session='{}'",
            req.query, req.session_id
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
            let search_results = if req.session_id.is_empty() {
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
            .map_err(|e| Status::not_found(e))?;
        let session = session_arc.load();

        use crate::summary::{SummaryGenerator, SummaryOptions};
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
            .map_err(|e| Status::not_found(e))?;
        let session = session_arc.load();

        use crate::summary::SummaryGenerator;
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
            .map_err(|e| Status::not_found(e))?;
        let session = session_arc.load();

        use crate::summary::SummaryGenerator;
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
            .map_err(|e| Status::not_found(e))?;
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
            .map_err(|e| Status::not_found(e))?;
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
            .map_err(|e| Status::not_found(e))?;
        let session = session_arc.load();

        use crate::summary::SummaryGenerator;
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
        debug!("gRPC AssembleContext: session_id={}, query={}", req.session_id, req.query);

        let session_id = parse_uuid(&req.session_id)?;
        let session_arc = self.memory.get_session(session_id).await
            .map_err(|e| Status::not_found(e))?;
        let session = session_arc.load();

        let token_budget = if req.token_budget == 0 { 4000 } else { req.token_budget as usize };

        // Collect all context updates from hot + warm context
        let updates: Vec<_> = session.hot_context.iter().iter()
            .chain(session.warm_context.iter().map(|c| &c.update))
            .cloned()
            .collect();

        use crate::core::context_assembly;

        let assembled = context_assembly::assemble_context(
            &req.query,
            &session.entity_graph,
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
            .map_err(|e| Status::not_found(e))?;

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
            .map_err(|e| Status::not_found(e))?;

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

    // --- Source Tracking (Phase 9) ---

    async fn register_source(
        &self,
        request: Request<RegisterSourceRequest>,
    ) -> Result<Response<RegisterSourceAck>, Status> {
        let session_id_str = get_session_id_from_metadata(&request)?;
        let session_id = parse_uuid(&session_id_str)?;

        let req = request.into_inner();

        // Ensure source_ref is present
        let source_ref = req
            .source_ref
            .ok_or_else(|| Status::invalid_argument("Missing source_ref in request"))?;

        debug!("gRPC RegisterSource: entry_id={}", source_ref.entry_id);

        match self
            .memory
            .storage_actor
            .register_source(session_id, source_ref)
            .await
        {
            Ok(()) => Ok(Response::new(RegisterSourceAck {})),
            Err(e) => {
                error!("gRPC RegisterSource failed: {}", e);
                let e_msg: String = e.to_string();
                Err(Status::internal(e_msg))
            }
        }
    }

    async fn register_source_batch(
        &self,
        request: Request<RegisterSourceBatchRequest>,
    ) -> Result<Response<RegisterSourceBatchAck>, Status> {
        let session_id_str = get_session_id_from_metadata(&request)?;
        let session_id = parse_uuid(&session_id_str)?;

        let req = request.into_inner();
        let total = req.sources.len();
        debug!(
            "gRPC RegisterSourceBatch: session={} sources={}",
            session_id, total
        );

        let futures = req.sources.into_iter().map(|source_ref| {
            let entry_id = source_ref.entry_id.clone();
            let actor = self.memory.storage_actor.clone();
            async move { (entry_id, actor.register_source(session_id, source_ref).await) }
        });
        let results = join_all(futures).await;

        let mut registered: u32 = 0;
        for (entry_id, result) in results {
            match result {
                Ok(()) => registered += 1,
                Err(e) => {
                    error!(
                        "gRPC RegisterSourceBatch: failed for entry_id={}: {}",
                        entry_id, e
                    );
                }
            }
        }

        debug!(
            "gRPC RegisterSourceBatch: registered {}/{} sources",
            registered, total
        );
        Ok(Response::new(RegisterSourceBatchAck { registered }))
    }

    async fn check_freshness(
        &self,
        request: Request<FreshnessRequest>,
    ) -> Result<Response<FreshnessReport>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC CheckFreshness: checking {} entries",
            req.entry_ids.len()
        );

        if req.entry_ids.len() != req.current_hashes.len() {
            return Err(Status::invalid_argument(
                "entry_ids and current_hashes must have the same length",
            ));
        }

        let has_checks = req.checks.len() == req.entry_ids.len();

        // Collect per-entry metadata needed for fallback Unknown responses.
        // (file_path is not stored in storage; it comes from the request.)
        let mut file_paths: Vec<String> = Vec::with_capacity(req.entry_ids.len());
        let mut fallback_hashes: Vec<Vec<u8>> = Vec::with_capacity(req.entry_ids.len());

        // Build the batch input: one tuple per entry.
        let batch: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)> = req
            .entry_ids
            .iter()
            .enumerate()
            .map(|(i, entry_id)| {
                let current_hash = req.current_hashes[i].hash.clone();
                file_paths.push(req.current_hashes[i].file_path.clone());
                fallback_hashes.push(current_hash.clone());

                let (ast_hash, symbol_name) = if has_checks {
                    let check = &req.checks[i];
                    (
                        if check.ast_hash.is_empty() {
                            None
                        } else {
                            Some(check.ast_hash.clone())
                        },
                        if check.symbol_name.is_empty() {
                            None
                        } else {
                            Some(check.symbol_name.clone())
                        },
                    )
                } else {
                    (None, None)
                };

                (entry_id.clone(), current_hash, ast_hash, symbol_name)
            })
            .collect();

        // Single actor message — one SurrealDB round-trip for the whole batch.
        match self.memory.storage_actor.check_freshness_batch(batch).await {
            Ok(entries) => Ok(Response::new(FreshnessReport { entries })),
            Err(e) => {
                error!("gRPC CheckFreshness batch failed: {}", e);
                // Fall back to Unknown for every entry so callers are not blocked.
                let reports = req
                    .entry_ids
                    .into_iter()
                    .enumerate()
                    .map(|(i, entry_id)| FreshnessEntry {
                        entry_id,
                        file_path: file_paths.get(i).cloned().unwrap_or_default(),
                        status: FreshnessStatus::Unknown as i32,
                        stored_hash: Vec::new(),
                        current_hash: fallback_hashes.get(i).cloned().unwrap_or_default(),
                    })
                    .collect();
                Ok(Response::new(FreshnessReport { entries: reports }))
            }
        }
    }

    async fn invalidate(
        &self,
        request: Request<InvalidateRequest>,
    ) -> Result<Response<InvalidateAck>, Status> {
        let req = request.into_inner();
        debug!("gRPC Invalidate: checking source path {}", req.file_path);

        match self
            .memory
            .storage_actor
            .invalidate_source(&req.file_path)
            .await
        {
            Ok(count) => Ok(Response::new(InvalidateAck {
                entries_invalidated: count,
            })),
            Err(e) => {
                error!("gRPC Invalidate failed for file {}: {}", req.file_path, e);
                let e_msg: String = e.to_string();
                Err(Status::internal(e_msg))
            }
        }
    }

    async fn register_symbol_dependency(
        &self,
        request: Request<RegisterSymbolDependencyRequest>,
    ) -> Result<Response<RegisterSymbolDependencyAck>, Status> {
        let req = request.into_inner();
        let from = req
            .from_symbol
            .ok_or_else(|| Status::invalid_argument("Missing from_symbol"))?;
        let to_symbols = req.to_symbols;

        debug!(
            "gRPC RegisterSymbolDependency: {}::{} -> {} deps",
            from.file_path,
            from.symbol_name,
            to_symbols.len()
        );

        match self
            .memory
            .storage_actor
            .register_symbol_dependencies(from, to_symbols)
            .await
        {
            Ok(count) => Ok(Response::new(RegisterSymbolDependencyAck {
                edges_created: count,
            })),
            Err(e) => {
                error!("gRPC RegisterSymbolDependency failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn cascade_invalidate(
        &self,
        request: Request<CascadeInvalidateRequest>,
    ) -> Result<Response<CascadeInvalidateReport>, Status> {
        let req = request.into_inner();
        let changed = req
            .changed_symbol
            .ok_or_else(|| Status::invalid_argument("Missing changed_symbol"))?;
        let max_depth = if req.max_depth == 0 { 10 } else { req.max_depth };

        debug!(
            "gRPC CascadeInvalidate: {}::{} depth={}",
            changed.file_path, changed.symbol_name, max_depth
        );

        match self
            .memory
            .storage_actor
            .cascade_invalidate(changed, req.new_ast_hash, max_depth)
            .await
        {
            Ok(report) => Ok(Response::new(report)),
            Err(e) => {
                error!("gRPC CascadeInvalidate failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }
}

// --- Helpers ---

fn parse_uuid(s: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s).map_err(|_| Status::invalid_argument(format!("Invalid UUID: {s}")))
}

fn get_session_id_from_metadata<T>(request: &Request<T>) -> Result<String, Status> {
    request
        .metadata()
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| Status::unauthenticated("Missing x-session-id metadata"))
}

fn parse_session_role(s: &str) -> SessionRole {
    match s.to_lowercase().as_str() {
        "primary" => SessionRole::Primary,
        "dependency" => SessionRole::Dependency,
        "shared" => SessionRole::Shared,
        _ => SessionRole::Related,
    }
}

fn workspace_to_info(workspace: &crate::workspace::Workspace) -> WorkspaceInfo {
    let created_at_unix = workspace
        .created_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let sessions: Vec<WorkspaceSessionEntry> = workspace
        .session_ids
        .iter()
        .map(|entry| WorkspaceSessionEntry {
            session_id: entry.key().to_string(),
            role: format!("{:?}", entry.value()),
        })
        .collect();

    WorkspaceInfo {
        workspace_id: workspace.id.to_string(),
        name: workspace.name.clone(),
        description: workspace.description.clone(),
        created_at_unix,
        sessions,
    }
}

fn format_context_description(interaction_type: &str, content: &ContextContent) -> String {
    let mut desc = String::new();
    if !content.title.is_empty() {
        desc.push_str(&content.title);
    }
    if !content.description.is_empty() {
        if !desc.is_empty() {
            desc.push_str(": ");
        }
        desc.push_str(&content.description);
    }
    if desc.is_empty() {
        desc = format!("[{interaction_type}] update");
    }
    desc
}

fn build_update_metadata(interaction_type: &str, content: &ContextContent) -> serde_json::Value {
    use crate::core::context_update::{ContextUpdate, UpdateContent, UpdateType};

    let update_type = match interaction_type {
        "decision_made" => UpdateType::DecisionMade,
        "problem_solved" => UpdateType::ProblemSolved,
        "code_change" | "code_changed" => UpdateType::CodeChanged,
        "qa" | "question_answered" => UpdateType::QuestionAnswered,
        "requirement_added" => UpdateType::RequirementAdded,
        "concept_defined" | _ => UpdateType::ConceptDefined,
    };

    let related_code = content.code_ref.as_ref().map(|c| {
        crate::core::context_update::CodeReference {
            file_path: c.file_path.clone(),
            start_line: c.start_line,
            end_line: c.end_line,
            code_snippet: c.code_snippet.clone(),
            commit_hash: if c.commit_hash.is_empty() { None } else { Some(c.commit_hash.clone()) },
            branch: if c.branch.is_empty() { None } else { Some(c.branch.clone()) },
            change_description: c.change_description.clone(),
        }
    });

    let update = ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type,
        content: UpdateContent {
            title: content.title.clone(),
            description: content.description.clone(),
            details: content.details.clone(),
            examples: content.examples.clone(),
            implications: content.implications.clone(),
        },
        related_code,
        parent_update: None,
        user_marked_important: false,
        creates_entities: Vec::new(),
        creates_relationships: Vec::new(),
        references_entities: Vec::new(),
    };

    serde_json::to_value(update).expect("ContextUpdate serialization cannot fail")
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
