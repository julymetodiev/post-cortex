// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Canonical [`PostCortexService`] implementation for post-cortex-memory.
//!
//! [`MemoryServiceImpl`] is a thin adapter over
//! [`ConversationMemorySystem`] that fulfils the
//! [`post_cortex_core::services::PostCortexService`] trait. Phases 6 + 7
//! migrate the daemon's gRPC handlers and the MCP tool layer to call
//! this trait instead of `ConversationMemorySystem` directly — at that
//! point every read / write / search / manage operation flows through
//! one canonical function (TODO.md:106-117).
//!
//! Right now this impl is **incomplete on purpose**: most methods return
//! a `SystemError::Internal("not yet wired …")` while the legacy
//! transport handlers continue to call `ConversationMemorySystem` paths
//! directly. The trait surface is stable; the impl fills in
//! incrementally as Phases 6 + 7 migrate their callers. This avoids a
//! single massive commit that's impossible to bisect.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use post_cortex_core::core::context_update::{ContextUpdate, EntityRelationship, TypedEntity};
use post_cortex_core::core::error::SystemError;
use post_cortex_core::services::{
    AdminRequest, AdminResponse, AssembleContextRequest, AssembleContextResponse,
    BulkUpdateContextRequest, BulkUpdateContextResponse, HealthReport, ManageEntityRequest,
    ManageEntityResponse, ManageSessionRequest, ManageSessionResponse, ManageWorkspaceRequest,
    ManageWorkspaceResponse, PostCortexService, QueryContextRequest, QueryContextResponse,
    SemanticSearchRequest, SemanticSearchResponse, StructuredSummaryRequest,
    StructuredSummaryResponse, UpdateContextRequest, UpdateContextResponse,
};
use tracing::warn;
use uuid::Uuid;

use crate::memory_system::ConversationMemorySystem;
use crate::pipeline::{
    EmbeddingWorkItem, GraphWorkItem, Pipeline, PipelineConfig, PipelineError, SummaryWorkItem,
};

/// Canonical [`PostCortexService`] implementation backed by
/// [`ConversationMemorySystem`].
///
/// Construct with [`Self::new`] passing an `Arc<ConversationMemorySystem>`
/// (typically the singleton owned by the daemon).
pub struct MemoryServiceImpl {
    system: Arc<ConversationMemorySystem>,
    pipeline: Arc<Pipeline>,
}

impl MemoryServiceImpl {
    /// Wrap an existing memory system in the canonical service trait,
    /// starting the non-blocking write pipeline with default capacities.
    /// Must be called from inside a Tokio runtime.
    #[must_use]
    pub fn new(system: Arc<ConversationMemorySystem>) -> Self {
        Self::with_pipeline_config(system, PipelineConfig::default())
    }

    /// Wrap an existing memory system in the service trait with an
    /// explicit pipeline configuration.
    #[must_use]
    pub fn with_pipeline_config(
        system: Arc<ConversationMemorySystem>,
        config: PipelineConfig,
    ) -> Self {
        let pipeline = Arc::new(Pipeline::start(config, Arc::clone(&system)));
        Self { system, pipeline }
    }

    /// Borrow the non-blocking pipeline. Exposed so transport adapters
    /// can submit derived-work items directly (e.g. enqueue an
    /// embedding compute after a manual storage write).
    #[must_use]
    pub fn pipeline(&self) -> &Arc<Pipeline> {
        &self.pipeline
    }

    /// Borrow the underlying memory system. Provided so the daemon's
    /// gRPC handlers can fall back to direct calls for methods that
    /// have not been migrated to the trait yet.
    #[must_use]
    pub fn inner(&self) -> &Arc<ConversationMemorySystem> {
        &self.system
    }

    fn not_yet_wired<T>(op: &'static str) -> Result<T, SystemError> {
        Err(SystemError::Internal(format!(
            "PostCortexService::{op} is not yet wired — migration lands in Phase 6 (MCP) / Phase 7 (daemon). \
             Use ConversationMemorySystem directly until then."
        )))
    }
}

#[async_trait]
impl PostCortexService for MemoryServiceImpl {
    #[tracing::instrument(skip(self), name = "post_cortex.health")]
    async fn health(&self) -> Result<HealthReport, SystemError> {
        let health = self.system.get_system_health();
        Ok(HealthReport {
            status: if health.circuit_breaker_open {
                "degraded".to_string()
            } else {
                "ok".to_string()
            },
            active_sessions: health.active_sessions,
            // SystemHealth does not yet expose memory bytes — wired in
            // Phase 11 (perf observability). For now report 0.
            memory_usage_bytes: 0,
            pipeline_backlog: self.pipeline.backlog(),
            uptime_seconds: health.uptime_seconds,
        })
    }

    #[tracing::instrument(
        skip(self, req),
        fields(
            session_id = %req.session_id,
            interaction_type = ?req.interaction_type,
            entities = req.entities.len(),
            relations = req.relations.len(),
        ),
        name = "post_cortex.update_context",
    )]
    async fn update_context(
        &self,
        req: UpdateContextRequest,
    ) -> Result<UpdateContextResponse, SystemError> {
        validate_update_request(&req)?;

        let description = build_description(&req);
        let context_update = build_context_update(&req);
        let metadata = serde_json::to_value(&context_update)
            .expect("ContextUpdate serialization cannot fail");
        let session_id = req.session_id;

        let entry_id_str = self
            .system
            .add_incremental_update(session_id, description.clone(), Some(metadata))
            .await
            .map_err(SystemError::Internal)?;

        let entry_id = Uuid::parse_str(&entry_id_str).map_err(|e| {
            SystemError::Internal(format!(
                "storage returned non-UUID entry id {entry_id_str:?}: {e}"
            ))
        })?;

        // Hand derived work to the bounded pipeline. Best-effort: a full
        // queue logs `Backpressure` but doesn't fail the durable write
        // — the legacy in-system `tokio::spawn`s in
        // `ConversationMemorySystem::add_incremental_update_internal`
        // remain as the safety net until they're fully retired in 0.4.0.
        submit_derived_work(
            &self.pipeline,
            session_id,
            entry_id,
            &description,
            context_update,
        );

        Ok(UpdateContextResponse {
            entry_id,
            session_id,
            persisted_at: Utc::now(),
            durable: true,
        })
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %req.session_id, batch_size = req.updates.len()),
        name = "post_cortex.bulk_update_context",
    )]
    async fn bulk_update_context(
        &self,
        req: BulkUpdateContextRequest,
    ) -> Result<BulkUpdateContextResponse, SystemError> {
        // Pre-validate the whole batch first so a bad item doesn't leave us
        // with a half-applied write (storage-actor batching lands later;
        // until then we still fail-fast on validation but persist
        // sequentially).
        for (i, item) in req.updates.iter().enumerate() {
            if item.session_id != req.session_id {
                return Err(SystemError::InvalidArgument(format!(
                    "bulk_update_context: updates[{i}].session_id {} does not match request session_id {}",
                    item.session_id, req.session_id
                )));
            }
            validate_update_request(item).map_err(|e| match e {
                SystemError::InvalidArgument(msg) => {
                    SystemError::InvalidArgument(format!("updates[{i}]: {msg}"))
                }
                other => other,
            })?;
        }

        let mut entry_ids = Vec::with_capacity(req.updates.len());
        for (i, item) in req.updates.iter().enumerate() {
            let description = build_description(item);
            let context_update = build_context_update(item);
            let metadata = serde_json::to_value(&context_update)
                .expect("ContextUpdate serialization cannot fail");
            let entry_id_str = self
                .system
                .add_incremental_update(item.session_id, description.clone(), Some(metadata))
                .await
                .map_err(|e| SystemError::Internal(format!("updates[{i}]: {e}")))?;
            let entry_id = Uuid::parse_str(&entry_id_str).map_err(|e| {
                SystemError::Internal(format!(
                    "updates[{i}]: storage returned non-UUID entry id {entry_id_str:?}: {e}"
                ))
            })?;
            entry_ids.push(entry_id);
            submit_derived_work(
                &self.pipeline,
                item.session_id,
                entry_id,
                &description,
                context_update,
            );
        }

        Ok(BulkUpdateContextResponse {
            entry_ids,
            persisted_at: Utc::now(),
            durable: true,
        })
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.semantic_search")]
    async fn semantic_search(
        &self,
        _req: SemanticSearchRequest,
    ) -> Result<SemanticSearchResponse, SystemError> {
        Self::not_yet_wired("semantic_search")
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.query_context")]
    async fn query_context(
        &self,
        _req: QueryContextRequest,
    ) -> Result<QueryContextResponse, SystemError> {
        Self::not_yet_wired("query_context")
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.assemble_context")]
    async fn assemble_context(
        &self,
        _req: AssembleContextRequest,
    ) -> Result<AssembleContextResponse, SystemError> {
        Self::not_yet_wired("assemble_context")
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.manage_session")]
    async fn manage_session(
        &self,
        _req: ManageSessionRequest,
    ) -> Result<ManageSessionResponse, SystemError> {
        Self::not_yet_wired("manage_session")
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.manage_workspace")]
    async fn manage_workspace(
        &self,
        _req: ManageWorkspaceRequest,
    ) -> Result<ManageWorkspaceResponse, SystemError> {
        Self::not_yet_wired("manage_workspace")
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.manage_entity")]
    async fn manage_entity(
        &self,
        _req: ManageEntityRequest,
    ) -> Result<ManageEntityResponse, SystemError> {
        Self::not_yet_wired("manage_entity")
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.get_structured_summary")]
    async fn get_structured_summary(
        &self,
        _req: StructuredSummaryRequest,
    ) -> Result<StructuredSummaryResponse, SystemError> {
        Self::not_yet_wired("get_structured_summary")
    }

    #[tracing::instrument(skip(self, _req), name = "post_cortex.admin")]
    async fn admin(&self, _req: AdminRequest) -> Result<AdminResponse, SystemError> {
        Self::not_yet_wired("admin")
    }
}

// ---------------------------------------------------------------------------
// Canonical helpers — single source of truth for update_context validation
// and metadata shaping. Every transport (gRPC, MCP, REST) reaches the same
// behaviour by going through `update_context` / `bulk_update_context`; the
// helpers below are intentionally private so transports cannot bypass them.
// ---------------------------------------------------------------------------

/// Strict validation lifted from the legacy gRPC helper. Rejects any input
/// that would corrupt the entity graph (empty title+description, missing
/// entities/relations, dangling relation endpoints, self-relations, empty
/// relation context).
fn validate_update_request(req: &UpdateContextRequest) -> Result<(), SystemError> {
    if req.content.title.trim().is_empty() && req.content.description.trim().is_empty() {
        return Err(SystemError::InvalidArgument(
            "update_context: title and description are both empty — provide at least one".into(),
        ));
    }
    if req.entities.is_empty() {
        return Err(SystemError::InvalidArgument(
            "update_context: entities must not be empty".into(),
        ));
    }
    if req.relations.is_empty() {
        return Err(SystemError::InvalidArgument(
            "update_context: relations must not be empty".into(),
        ));
    }

    let entity_names: HashSet<&str> = req.entities.iter().map(|e| e.name.as_str()).collect();
    for (i, rel) in req.relations.iter().enumerate() {
        if rel.from_entity.is_empty() {
            return Err(SystemError::InvalidArgument(format!(
                "relation[{i}]: from_entity must not be empty"
            )));
        }
        if rel.to_entity.is_empty() {
            return Err(SystemError::InvalidArgument(format!(
                "relation[{i}]: to_entity must not be empty"
            )));
        }
        if rel.from_entity == rel.to_entity {
            return Err(SystemError::InvalidArgument(format!(
                "relation[{i}]: self-relations are not allowed (from_entity == to_entity == {:?})",
                rel.from_entity
            )));
        }
        if rel.context.trim().is_empty() {
            return Err(SystemError::InvalidArgument(format!(
                "relation[{i}]: context must not be empty — every relation requires an explanation"
            )));
        }
        if !entity_names.contains(rel.from_entity.as_str()) {
            return Err(SystemError::InvalidArgument(format!(
                "relation[{i}]: from_entity {:?} is not declared in the entities list",
                rel.from_entity
            )));
        }
        if !entity_names.contains(rel.to_entity.as_str()) {
            return Err(SystemError::InvalidArgument(format!(
                "relation[{i}]: to_entity {:?} is not declared in the entities list",
                rel.to_entity
            )));
        }
    }

    Ok(())
}

/// Build the text fed to the vectorizer — `title\n description` matches what
/// the MCP path settled on for cross-session search quality.
fn build_description(req: &UpdateContextRequest) -> String {
    if req.content.description.is_empty() {
        req.content.title.clone()
    } else if req.content.title.is_empty() {
        req.content.description.clone()
    } else {
        format!("{}\n{}", req.content.title, req.content.description)
    }
}

/// Build the `ContextUpdate` JSON blob stored as the storage entry's metadata.
/// Keeps `creates_entities` (names) and `typed_entities` (name + type) in sync
/// — both are consumed downstream by the entity graph builder.
fn build_context_update(req: &UpdateContextRequest) -> ContextUpdate {
    let typed_entities: Vec<TypedEntity> = req
        .entities
        .iter()
        .map(|e| TypedEntity {
            name: e.name.clone(),
            entity_type: e.entity_type.clone(),
        })
        .collect();
    let creates_entities: Vec<String> = req.entities.iter().map(|e| e.name.clone()).collect();
    let creates_relationships: Vec<EntityRelationship> = req.relations.clone();
    let related_code = req.code_reference.clone();

    ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: req.interaction_type.clone(),
        content: req.content.clone(),
        related_code,
        parent_update: None,
        user_marked_important: false,
        creates_entities,
        creates_relationships,
        references_entities: Vec::new(),
        typed_entities,
    }
}

/// Hand a freshly-persisted update off to the bounded background pipeline:
/// embedding compute, entity-graph merge, and summary refresh all run on
/// separate worker tasks so the write path returns as soon as the entry
/// is durably stored. Backpressure on any queue is logged but swallowed
/// — the legacy in-system `tokio::spawn`s in
/// [`ConversationMemorySystem::add_incremental_update_internal`] still
/// run as a safety net, so a full queue degrades to "do the work on a
/// raw spawn anyway" rather than dropping it.
fn submit_derived_work(
    pipeline: &Pipeline,
    session_id: Uuid,
    entry_id: Uuid,
    text: &str,
    update: ContextUpdate,
) {
    if let Err(e) = pipeline.submit_embedding(EmbeddingWorkItem {
        session_id,
        entry_id,
        text: text.to_string(),
    }) {
        log_pipeline_submit("embedding", session_id, entry_id, e);
    }
    if let Err(e) = pipeline.submit_graph(GraphWorkItem::ApplyUpdate {
        session_id,
        update,
    }) {
        log_pipeline_submit("graph", session_id, entry_id, e);
    }
    if let Err(e) = pipeline.submit_summary(SummaryWorkItem { session_id }) {
        log_pipeline_submit("summary", session_id, entry_id, e);
    }
}

fn log_pipeline_submit(queue: &str, session_id: Uuid, entry_id: Uuid, err: PipelineError) {
    warn!(
        queue,
        %session_id,
        %entry_id,
        error = %err,
        "pipeline submission failed (non-fatal — legacy in-system spawn covers the work)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_system::SystemConfig;
    use chrono::Utc;
    use post_cortex_core::core::context_update::{
        EntityData, EntityType, RelationType, UpdateContent, UpdateType,
    };

    fn entity(name: &str) -> EntityData {
        EntityData {
            name: name.to_string(),
            entity_type: EntityType::Concept,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 1,
            importance_score: 1.0,
            description: None,
        }
    }

    fn relation(from: &str, to: &str) -> EntityRelationship {
        EntityRelationship {
            from_entity: from.to_string(),
            to_entity: to.to_string(),
            relation_type: RelationType::RelatedTo,
            context: "test relation".to_string(),
        }
    }

    fn good_request() -> UpdateContextRequest {
        UpdateContextRequest {
            session_id: Uuid::new_v4(),
            interaction_type: UpdateType::ConceptDefined,
            content: UpdateContent {
                title: "Some concept".into(),
                description: "A short definition".into(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            entities: vec![entity("Foo"), entity("Bar")],
            relations: vec![relation("Foo", "Bar")],
            code_reference: None,
        }
    }

    #[tokio::test]
    async fn trait_is_object_safe() {
        fn _accept_dyn(_svc: Arc<dyn PostCortexService>) {}
    }

    #[test]
    fn validation_rejects_empty_title_and_description() {
        let mut req = good_request();
        req.content.title = String::new();
        req.content.description = String::new();
        let err = validate_update_request(&req).unwrap_err();
        assert!(matches!(err, SystemError::InvalidArgument(ref m) if m.contains("both empty")));
    }

    #[test]
    fn validation_rejects_empty_entities() {
        let mut req = good_request();
        req.entities = vec![];
        assert!(matches!(
            validate_update_request(&req),
            Err(SystemError::InvalidArgument(_))
        ));
    }

    #[test]
    fn validation_rejects_empty_relations() {
        let mut req = good_request();
        req.relations = vec![];
        assert!(matches!(
            validate_update_request(&req),
            Err(SystemError::InvalidArgument(_))
        ));
    }

    #[test]
    fn validation_rejects_self_relation() {
        let mut req = good_request();
        req.relations = vec![relation("Foo", "Foo")];
        assert!(matches!(
            validate_update_request(&req),
            Err(SystemError::InvalidArgument(ref m)) if m.contains("self-relations")
        ));
    }

    #[test]
    fn validation_rejects_dangling_relation_endpoint() {
        let mut req = good_request();
        req.relations = vec![relation("Foo", "Ghost")];
        assert!(matches!(
            validate_update_request(&req),
            Err(SystemError::InvalidArgument(ref m)) if m.contains("Ghost")
        ));
    }

    #[test]
    fn validation_rejects_empty_relation_context() {
        let mut req = good_request();
        req.relations[0].context = "   ".into();
        assert!(matches!(
            validate_update_request(&req),
            Err(SystemError::InvalidArgument(ref m)) if m.contains("context must not be empty")
        ));
    }

    #[test]
    fn validation_accepts_good_request() {
        assert!(validate_update_request(&good_request()).is_ok());
    }

    #[test]
    fn description_joins_title_and_body() {
        let req = good_request();
        assert_eq!(build_description(&req), "Some concept\nA short definition");
    }

    #[test]
    fn description_falls_back_when_one_side_empty() {
        let mut req = good_request();
        req.content.description = String::new();
        assert_eq!(build_description(&req), "Some concept");

        req.content.title = String::new();
        req.content.description = "Body only".into();
        assert_eq!(build_description(&req), "Body only");
    }

    #[test]
    fn metadata_keeps_creates_entities_in_sync_with_typed_entities() {
        let req = good_request();
        let update = build_context_update(&req);
        let meta = serde_json::to_value(&update).unwrap();
        let names = meta["creates_entities"].as_array().unwrap();
        let typed = meta["typed_entities"].as_array().unwrap();
        assert_eq!(names.len(), typed.len());
        assert_eq!(names.len(), req.entities.len());
        for (i, e) in req.entities.iter().enumerate() {
            assert_eq!(names[i].as_str().unwrap(), e.name);
            assert_eq!(typed[i]["name"].as_str().unwrap(), e.name);
        }
    }

    async fn make_service(suffix: &str) -> (MemoryServiceImpl, String) {
        let test_dir = format!(
            "./test_data_memservice_{}_{}",
            suffix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::fs::create_dir_all(&test_dir).unwrap();
        let config = SystemConfig {
            data_directory: test_dir.clone(),
            ..Default::default()
        };
        let system = Arc::new(ConversationMemorySystem::new(config).await.unwrap());
        (MemoryServiceImpl::new(system), test_dir)
    }

    #[tokio::test]
    async fn health_returns_ok_status() {
        let (svc, test_dir) = make_service("health").await;
        let report = svc.health().await.unwrap();
        assert!(report.status == "ok" || report.status == "degraded");
        std::fs::remove_dir_all(&test_dir).unwrap();
    }

    #[tokio::test]
    async fn update_context_persists_and_returns_entry_id() {
        let (svc, test_dir) = make_service("update").await;
        let session_id = svc.inner().create_session(None, None).await.unwrap();

        let mut req = good_request();
        req.session_id = session_id;
        let resp = svc.update_context(req).await.unwrap();

        assert_eq!(resp.session_id, session_id);
        assert!(resp.durable);
        std::fs::remove_dir_all(&test_dir).unwrap();
    }

    #[tokio::test]
    async fn update_context_rejects_invalid_input_with_invalid_argument() {
        let (svc, test_dir) = make_service("invalid").await;
        let session_id = svc.inner().create_session(None, None).await.unwrap();

        let mut req = good_request();
        req.session_id = session_id;
        req.entities = vec![]; // violates "entities must not be empty"
        let err = svc.update_context(req).await.unwrap_err();
        assert!(matches!(err, SystemError::InvalidArgument(_)));
        std::fs::remove_dir_all(&test_dir).unwrap();
    }

    #[tokio::test]
    async fn bulk_update_context_persists_every_item() {
        let (svc, test_dir) = make_service("bulk").await;
        let session_id = svc.inner().create_session(None, None).await.unwrap();

        let mut a = good_request();
        a.session_id = session_id;
        a.content.title = "First".into();
        let mut b = good_request();
        b.session_id = session_id;
        b.content.title = "Second".into();

        let resp = svc
            .bulk_update_context(BulkUpdateContextRequest {
                session_id,
                updates: vec![a, b],
            })
            .await
            .unwrap();
        assert_eq!(resp.entry_ids.len(), 2);
        assert!(resp.durable);
        std::fs::remove_dir_all(&test_dir).unwrap();
    }

    #[tokio::test]
    async fn update_context_returns_fast_then_pipeline_drains_in_background() {
        // The hot path persists durably; embedding + graph + summary land
        // on background workers. Verify that, on a warm path
        // (post-vectorizer-init), update_context returns quickly and the
        // pipeline backlog drains shortly after.
        //
        // The first call still pays the model-download cost via the
        // legacy in-system `spawn_background_vectorization` (kept as a
        // safety net while direct callers migrate to the pipeline path).
        // Phase 7+ removes that path entirely.
        let (svc, test_dir) = make_service("nonblocking").await;
        let session_id = svc.inner().create_session(None, None).await.unwrap();

        // Warm-up call — absorbs first-time model load.
        let mut warmup = good_request();
        warmup.session_id = session_id;
        warmup.content.title = "warmup".into();
        let _ = svc.update_context(warmup).await.unwrap();

        // Wait for any pending background work to settle.
        for _ in 0..100 {
            if svc.pipeline().backlog() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        // Measured call — should be fast.
        let mut req = good_request();
        req.session_id = session_id;
        let start = std::time::Instant::now();
        let resp = svc.update_context(req).await.unwrap();
        let write_latency = start.elapsed();

        assert!(resp.durable);
        assert!(
            write_latency.as_millis() < 250,
            "update_context took {write_latency:?} on warm path — should be <250ms"
        );

        // Workers drain the queues asynchronously.
        for _ in 0..100 {
            if svc.pipeline().backlog() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            svc.pipeline().backlog(),
            0,
            "pipeline backlog should drain within 2s"
        );

        std::fs::remove_dir_all(&test_dir).unwrap();
    }

    #[tokio::test]
    async fn bulk_update_context_rejects_mismatched_session() {
        let (svc, test_dir) = make_service("bulkmis").await;
        let session_id = svc.inner().create_session(None, None).await.unwrap();

        let mut item = good_request();
        item.session_id = Uuid::new_v4(); // intentionally different
        let err = svc
            .bulk_update_context(BulkUpdateContextRequest {
                session_id,
                updates: vec![item],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, SystemError::InvalidArgument(ref m) if m.contains("does not match")));
        std::fs::remove_dir_all(&test_dir).unwrap();
    }
}
