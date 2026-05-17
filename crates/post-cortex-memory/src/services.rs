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

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_core::core::error::SystemError;
use post_cortex_core::services::{
    AdminRequest, AdminResponse, AssembleContextRequest, AssembleContextResponse,
    BulkUpdateContextRequest, BulkUpdateContextResponse, HealthReport, ManageEntityRequest,
    ManageEntityResponse, ManageSessionRequest, ManageSessionResponse, ManageWorkspaceRequest,
    ManageWorkspaceResponse, PostCortexService, QueryContextRequest, QueryContextResponse,
    SemanticSearchRequest, SemanticSearchResponse, StructuredSummaryRequest,
    StructuredSummaryResponse, UpdateContextRequest, UpdateContextResponse,
};

use crate::memory_system::ConversationMemorySystem;
use crate::pipeline::{Pipeline, PipelineConfig};

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
        let pipeline = Arc::new(Pipeline::start(config));
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

    async fn update_context(
        &self,
        _req: UpdateContextRequest,
    ) -> Result<UpdateContextResponse, SystemError> {
        Self::not_yet_wired("update_context")
    }

    async fn bulk_update_context(
        &self,
        _req: BulkUpdateContextRequest,
    ) -> Result<BulkUpdateContextResponse, SystemError> {
        Self::not_yet_wired("bulk_update_context")
    }

    async fn semantic_search(
        &self,
        _req: SemanticSearchRequest,
    ) -> Result<SemanticSearchResponse, SystemError> {
        Self::not_yet_wired("semantic_search")
    }

    async fn query_context(
        &self,
        _req: QueryContextRequest,
    ) -> Result<QueryContextResponse, SystemError> {
        Self::not_yet_wired("query_context")
    }

    async fn assemble_context(
        &self,
        _req: AssembleContextRequest,
    ) -> Result<AssembleContextResponse, SystemError> {
        Self::not_yet_wired("assemble_context")
    }

    async fn manage_session(
        &self,
        _req: ManageSessionRequest,
    ) -> Result<ManageSessionResponse, SystemError> {
        Self::not_yet_wired("manage_session")
    }

    async fn manage_workspace(
        &self,
        _req: ManageWorkspaceRequest,
    ) -> Result<ManageWorkspaceResponse, SystemError> {
        Self::not_yet_wired("manage_workspace")
    }

    async fn manage_entity(
        &self,
        _req: ManageEntityRequest,
    ) -> Result<ManageEntityResponse, SystemError> {
        Self::not_yet_wired("manage_entity")
    }

    async fn get_structured_summary(
        &self,
        _req: StructuredSummaryRequest,
    ) -> Result<StructuredSummaryResponse, SystemError> {
        Self::not_yet_wired("get_structured_summary")
    }

    async fn admin(&self, _req: AdminRequest) -> Result<AdminResponse, SystemError> {
        Self::not_yet_wired("admin")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_system::SystemConfig;

    #[tokio::test]
    async fn trait_is_object_safe() {
        // Compile-time check that PostCortexService is object-safe.
        fn _accept_dyn(_svc: Arc<dyn PostCortexService>) {}
    }

    #[tokio::test]
    async fn health_returns_ok_status() {
        let test_dir = format!(
            "./test_data_memservice_{}",
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
        let svc = MemoryServiceImpl::new(system);

        let report = svc.health().await.unwrap();
        assert!(report.status == "ok" || report.status == "degraded");

        std::fs::remove_dir_all(&test_dir).unwrap();
    }
}
