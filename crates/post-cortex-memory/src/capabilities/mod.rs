// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The memory capability services (docs/ARCHITECTURE.md section 4): the
//! API of the memory crate. Each service has one method per operation, with
//! that operation's validation, and one error type. [`Memory`] builds them
//! all over one runtime (session cache, workspace registry, persister,
//! pipeline, models), owned once and shared by `Arc`.
//!
//! Transports (MCP, gRPC, REST), `pcx` and the cloud call these services;
//! nothing outside the crate reaches the runtime or the storage ports.

use std::sync::Arc;

use post_cortex_core::redact::Redactor;
use post_cortex_ports::Stores;
use post_cortex_ports::{ArchiveCodec, BlobStore, EmbeddingEngine, Reranker, SearchIndex};

use crate::memory_system::{MemoryRuntime, SystemConfig};
use crate::pipeline::{Pipeline, PipelineConfig};
use derived::DerivedQueue;

mod admin;
mod assembly;
mod blobs;
mod cards;
mod changes;
mod derived;
mod documents;
mod entities;
mod events;
mod freshness;
mod messages;
mod projects;
pub(crate) mod records;
mod scope;
mod search;
mod sessions;
mod telling;
mod transfer;
mod views;
mod workspaces;

pub use admin::{Admin, AdminError, Health, PassCounts};
pub use assembly::{AssembleRequest, Assembly, AssemblyError, DEFAULT_TOKEN_BUDGET};
pub use blobs::{BlobError, Blobs, Swept};
pub use cards::{CardError, Cards};
pub use changes::{Changes, MemoryWrites, Written};
pub use documents::{
    DocumentError, DocumentFilter, DocumentPage, Documents, ListDocuments, Migrated, PutDocument,
    PutOutcome, Writer,
};
pub use entities::{Entities, EntityError};
pub use events::{EventError, Events, Followed};
pub use freshness::{Freshness, FreshnessCheck, FreshnessError};
pub use messages::{MessageError, Messages};
pub use projects::{ProjectError, ProjectPlacement, Projects};
pub use records::{ListRecords, RecordError, RecordPage, Records};
pub use scope::{ScopeError, Target};
pub use search::{Found, Search, SearchError, SearchScope};
pub use sessions::{SessionError, Sessions};
pub use telling::{FileTelling, Moment, Telling};
pub use transfer::{
    ArchiveFile, ArchivePorts, ArchiveSpool, CardHead, CardHeads, ConflictResolution, ExportPlan,
    ImportConflict, ImportPreview, Transfer, TransferError,
};
pub use views::{
    AUTHOR_SCAN, ActivityEvent, ActivityKind, ActivityStream, Compaction, ContextQuery,
    ContextResponse, DEFAULT_HITS, DEFAULT_IMPORTANCE_LIMIT, DEFAULT_INSIGHTS_LIMIT,
    DEFAULT_NETWORK_ENTITIES, DEFAULT_NETWORK_RELATIONSHIPS, DetailViews, EntityImportance,
    EntityNetworkView, EntityRequest, ImportancePage, MAX_HITS, MAX_SUMMARY_TOKENS, NetworkEntity,
    NetworkRelationship, SearchDetail, SessionStatistics, StructuredSummary, SummaryRequest,
    SummarySection, Views, ViewsError, WatchRequest,
};
pub use workspaces::{
    CreateWorkspace, MAX_WORKSPACE_DESCRIPTION_CHARS, MAX_WORKSPACE_NAME_CHARS, UpdateWorkspace,
    WorkspaceError, Workspaces,
};

pub(crate) use workspaces::hydrate as hydrate_workspace;

/// Why a memory instance could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The storage backend could not be opened.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
    /// Embeddings are on but no embedding engine or search index was given.
    #[error("embeddings are enabled but the {0} is missing")]
    MissingPort(&'static str),
    /// The redaction configuration does not compile.
    #[error(transparent)]
    Redaction(#[from] post_cortex_core::redact::RedactError),
}

/// The ports a memory instance runs on, opened by the composition root
/// (the daemon, `pcx` admin commands, the cloud tenant) from its adapter
/// configuration (ARCHITECTURE section 5).
#[derive(Clone)]
pub struct MemoryPorts {
    /// The storage ports.
    pub stores: Stores,
    /// The search index; needed with embeddings on, and for the vectors of
    /// an export or import.
    pub index: Option<Arc<dyn SearchIndex>>,
    /// The embedding engine; needed with embeddings on. One engine may
    /// serve every instance of a process.
    pub engine: Option<Arc<dyn EmbeddingEngine>>,
    /// The reranker of `local` mode (and the fallback of `accurate`).
    pub reranker: Option<Arc<dyn Reranker>>,
    /// The reranker of `accurate` mode.
    pub accurate_reranker: Option<Arc<dyn Reranker>>,
    /// The export archive codec.
    pub archive: Arc<dyn ArchiveCodec>,
    /// The blob store (W24); none: this instance keeps no blobs, and a
    /// document that names one is refused.
    pub blobs: Option<Arc<dyn BlobStore>>,
}

/// Every capability service of one memory instance (see the module docs).
/// Each field is cheap to clone and can be handed to a transport alone.
#[derive(Clone)]
pub struct Memory {
    /// Write, bulk write, delete, forget, purge, change feed.
    pub records: Records,
    /// Semantic search.
    pub search: Search,
    /// Graph-aware context assembly.
    pub assembly: Assembly,
    /// Graph views, summaries, radar dismissals.
    pub views: Views,
    /// A file's memory told to an agent once per session.
    pub telling: FileTelling,
    /// The views the app shows (`pcx.views.v1`).
    pub detail_views: DetailViews,
    /// Entity delete and merge.
    pub entities: Entities,
    /// Session lifecycle and checkpoints.
    pub sessions: Sessions,
    /// Workspace lifecycle and membership.
    pub workspaces: Workspaces,
    /// Project resolution.
    pub projects: Projects,
    /// Project and user cards.
    pub cards: Cards,
    /// The images documents name.
    pub blobs: Blobs,
    /// The documents of modules.
    pub documents: Documents,
    /// Every change as storage holds it (ChangeLog).
    pub changes: Changes,
    /// Captured conversation turns.
    pub messages: Messages,
    /// The organisation's bus, for the host.
    pub events: Events,
    /// Source references and staleness.
    pub freshness: Freshness,
    /// Export and import.
    pub transfer: Transfer,
    /// Health, vectorization, flush.
    pub admin: Admin,
}

impl Memory {
    /// Build every service over `ports`, with the default pipeline. Must be
    /// called inside a Tokio runtime (the pipeline starts its workers).
    ///
    /// # Errors
    /// Embeddings are on without an engine or an index, or the runtime
    /// could not be built.
    pub async fn over_ports(ports: MemoryPorts, config: SystemConfig) -> Result<Self, OpenError> {
        Self::over_ports_with(ports, config, PipelineConfig::default()).await
    }

    /// [`Self::over_ports`] with an explicit pipeline configuration.
    ///
    /// # Errors
    /// As [`Self::over_ports`].
    pub async fn over_ports_with(
        ports: MemoryPorts,
        config: SystemConfig,
        pipeline: PipelineConfig,
    ) -> Result<Self, OpenError> {
        if config.enable_embeddings {
            if ports.engine.is_none() {
                return Err(OpenError::MissingPort("embedding engine"));
            }
            if ports.index.is_none() {
                return Err(OpenError::MissingPort("search index"));
            }
        }
        let redactor = Redactor::new(&config.redaction)?;
        let service_ports = ports.clone();
        let rt = MemoryRuntime::new(ports, config)
            .await
            .map_err(OpenError::Storage)?;
        Ok(Self::from_runtime(
            Arc::new(rt),
            &service_ports,
            pipeline,
            redactor,
        ))
    }

    /// The composition: one runtime, one pipeline and one redactor shared
    /// by every service.
    /// Each service gets the ports of its row (ARCHITECTURE rule 4); the
    /// runtime holds the session cache and the state the services share.
    fn from_runtime(
        rt: Arc<MemoryRuntime>,
        ports: &MemoryPorts,
        pipeline: PipelineConfig,
        redactor: Redactor,
    ) -> Self {
        let pipeline = Arc::new(Pipeline::start(pipeline, Arc::clone(&rt)));
        let redactor = Arc::new(redactor);
        let workspaces = Workspaces::new(
            Arc::clone(&ports.stores.workspaces),
            ports.blobs.clone(),
            Arc::clone(&rt.workspace_manager),
            Arc::clone(&rt.project_locks),
            Arc::clone(&rt.workspace_graphs),
            rt.changes.clone(),
        );
        let cards = Cards::new(
            Arc::clone(&ports.stores.cards),
            Arc::clone(&rt.workspace_manager),
            Arc::clone(&redactor),
            rt.changes.clone(),
        );
        let derived = DerivedQueue::new(Arc::clone(&pipeline), Arc::clone(&rt));
        let documents = Documents::new(
            Arc::clone(&rt),
            Arc::clone(&ports.stores.documents),
            ports.blobs.clone(),
            derived.clone(),
            Arc::clone(&redactor),
        );
        let search = Search::new(Arc::clone(&rt));
        let sessions = Sessions::new(Arc::clone(&rt), Arc::clone(&ports.stores.sessions));
        let messages = Messages::new(
            Arc::clone(&rt),
            Arc::clone(&ports.stores.messages),
            Arc::clone(&redactor),
            derived.clone(),
        );
        let views = Views::new(Arc::clone(&rt), workspaces.clone());
        Self {
            records: Records::new(Arc::clone(&rt), derived.clone(), redactor),
            assembly: Assembly::new(Arc::clone(&rt), cards.clone(), search.clone()),
            detail_views: DetailViews::new(
                views.clone(),
                sessions.clone(),
                messages.clone(),
                search.clone(),
            ),
            search,
            telling: FileTelling::new(views.clone()),
            views,
            entities: Entities::new(Arc::clone(&rt)),
            projects: Projects::new(Arc::clone(&rt), workspaces.clone(), sessions.clone()),
            sessions,
            workspaces,
            cards,
            blobs: Blobs::new(
                ports.blobs.clone(),
                documents.clone(),
                Arc::clone(&rt.workspace_manager),
            ),
            documents,
            changes: Changes::new(rt.changes.clone(), Arc::clone(&rt.workspace_manager)),
            messages,
            events: Events::new(
                Arc::clone(&ports.stores.events),
                Arc::clone(&ports.stores.cursors),
            ),
            freshness: Freshness::new(Arc::clone(&rt), Arc::clone(&ports.stores.freshness)),
            transfer: Transfer::new(Arc::clone(&rt), ports, derived.clone()),
            admin: Admin::new(rt, pipeline),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support;
