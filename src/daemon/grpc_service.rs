//! gRPC service for Post-Cortex
//!
//! Provides a tonic gRPC interface to ConversationMemorySystem,
//! enabling native binary protocol access for coding agents like Axon.

use crate::ConversationMemorySystem;
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

    // --- Source Tracking (stub for Phase 4, full implementation in Phase 9) ---

    async fn register_source(
        &self,
        _request: Request<RegisterSourceRequest>,
    ) -> Result<Response<RegisterSourceAck>, Status> {
        // TODO: Phase 9 — store source references for freshness tracking
        warn!("gRPC RegisterSource: not yet implemented (Phase 9)");
        Ok(Response::new(RegisterSourceAck {}))
    }

    async fn check_freshness(
        &self,
        _request: Request<FreshnessRequest>,
    ) -> Result<Response<FreshnessReport>, Status> {
        // TODO: Phase 9 — check file hashes against stored source references
        warn!("gRPC CheckFreshness: not yet implemented (Phase 9)");
        Ok(Response::new(FreshnessReport {
            entries: Vec::new(),
        }))
    }

    async fn invalidate(
        &self,
        _request: Request<InvalidateRequest>,
    ) -> Result<Response<InvalidateAck>, Status> {
        // TODO: Phase 9 — invalidate entries by file path
        warn!("gRPC Invalidate: not yet implemented (Phase 9)");
        Ok(Response::new(InvalidateAck {
            entries_invalidated: 0,
        }))
    }
}

// --- Helpers ---

fn parse_uuid(s: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s).map_err(|_| Status::invalid_argument(format!("Invalid UUID: {s}")))
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

fn build_update_metadata(
    interaction_type: &str,
    content: &ContextContent,
) -> serde_json::Value {
    let mut meta = serde_json::json!({
        "interaction_type": interaction_type,
        "title": content.title,
        "description": content.description,
    });

    if !content.details.is_empty() {
        meta["details"] = serde_json::json!(content.details);
    }
    if !content.examples.is_empty() {
        meta["examples"] = serde_json::json!(content.examples);
    }
    if !content.implications.is_empty() {
        meta["implications"] = serde_json::json!(content.implications);
    }

    if let Some(ref code_ref) = content.code_ref {
        meta["code_reference"] = serde_json::json!({
            "file_path": code_ref.file_path,
            "start_line": code_ref.start_line,
            "end_line": code_ref.end_line,
            "code_snippet": code_ref.code_snippet,
            "change_description": code_ref.change_description,
        });
    }

    meta
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
