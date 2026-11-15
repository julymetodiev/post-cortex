use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::OnceCell;
use tracing::{info, warn};

use crate::capabilities::MemoryPorts;
use crate::memory_system::RuntimeError;
use crate::memory_system::config::EmbeddingConfigHolder;
use crate::memory_system::config::SystemConfig;
use crate::memory_system::metrics::SystemMetrics;
use crate::memory_system::persister::{PersistSink, SessionPersister, StorageSink};
use crate::memory_system::session_manager::SessionManager;
use crate::memory_system::workspace_registry::WorkspaceManager;
use post_cortex_ports::{EmbeddingEngine, Reranker, SearchIndex, Stores};

/// The runtime state of one memory instance, owned once and shared by the
/// capability services (`crate::capabilities`): the session cache, the
/// workspace registry, the persister, the derived-work counter, the lazily
/// built vectorizer and the ports. Not public: the capability services are
/// the API.
pub struct MemoryRuntime {
    /// Session lifecycle manager with cache
    pub(crate) session_manager: SessionManager,
    /// Workspace management coordinator
    pub(crate) workspace_manager: Arc<WorkspaceManager>,
    /// The storage ports of the backend
    pub(crate) stores: Stores,
    /// The search index (vectors and lexical retrieval)
    index: Option<Arc<dyn SearchIndex>>,
    /// The embedding engine
    engine: Option<Arc<dyn EmbeddingEngine>>,
    /// The reranker of `local` mode
    reranker: Option<Arc<dyn Reranker>>,
    /// The reranker of `accurate` mode
    accurate_reranker: Option<Arc<dyn Reranker>>,
    /// The one writer path of session writes: ordered and coalesced per
    /// session (ARCHITECTURE section 5)
    pub(crate) persister: Arc<SessionPersister>,
    /// System configuration
    pub(crate) config: SystemConfig,
    /// Global system metrics
    pub(crate) system_metrics: Arc<SystemMetrics>,

    /// Lazily initialized content vectorizer
    pub(crate) content_vectorizer: Arc<OnceCell<Arc<crate::content_vectorizer::ContentVectorizer>>>,

    /// State of the lazy vectorizer start
    pub(crate) embedding_config_holder: Arc<EmbeddingConfigHolder>,

    /// Merged entity graphs of workspaces (bounded; invalidated on
    /// workspace delete and membership change)
    pub workspace_graphs: Arc<crate::memory_system::workspace_graphs::WorkspaceGraphCache>,

    /// Serialises project resolution per key and workspace changes per
    /// workspace
    pub project_locks: Arc<crate::memory_system::keyed_locks::KeyedLocks>,

    /// Embeddings and graph merges queued or running (see `flush_all`)
    pub derived_work: Arc<crate::memory_system::derived_work::DerivedWork>,

    /// Every change storage holds, as it is stored (ARCHITECTURE section 4,
    /// ChangeLog)
    pub changes: crate::memory_system::ChangeLog,
}

impl MemoryRuntime {
    /// Build the runtime over `ports`.
    ///
    /// # Errors
    /// The session cache could not be built.
    pub async fn new(ports: MemoryPorts, config: SystemConfig) -> Result<Self, RuntimeError> {
        let sink: Arc<dyn PersistSink> =
            Arc::new(StorageSink::new(Arc::clone(&ports.stores.sessions)));
        Self::new_with_sink(ports, sink, config).await
    }

    /// [`Self::new`] with session writes going to `sink` (a seam to inject
    /// storage failures).
    pub(crate) async fn new_with_sink(
        ports: MemoryPorts,
        sink: Arc<dyn PersistSink>,
        config: SystemConfig,
    ) -> Result<Self, RuntimeError> {
        let MemoryPorts {
            stores,
            index,
            engine,
            reranker,
            accurate_reranker,
            archive: _,
            blobs: _,
        } = ports;
        let changes = crate::memory_system::ChangeLog::new();
        let persister = SessionPersister::new(sink, changes.clone());
        let derived_work: Arc<crate::memory_system::derived_work::DerivedWork> = Arc::default();
        let session_manager = SessionManager::new(
            config.cache_capacity,
            Arc::clone(&stores.sessions),
            Arc::clone(&persister),
            Arc::clone(&derived_work),
        )?;

        let system_metrics = Arc::new(SystemMetrics::new());

        // Create workspace manager
        let workspace_manager = Arc::new(WorkspaceManager::default());

        // Hydrate workspaces from storage (a fresh store has none). A store
        // that cannot list them, or a row that cannot be read, fails the
        // start: an empty registry would hide them.
        let hydrate = stores.workspaces.list_workspaces().await?;
        tracing::info!("Hydrating {} workspaces from storage", hydrate.len());
        for stored in hydrate {
            crate::capabilities::hydrate_workspace(&workspace_manager, stored)?;
        }

        if config.enable_embeddings {
            info!("Embeddings enabled - will lazy-initialize on first use");
        } else {
            info!("Embeddings disabled in configuration");
        }
        let embedding_config_holder = Arc::new(EmbeddingConfigHolder {
            cross_session_search_enabled: config.enable_embeddings
                && config.cross_session_search_enabled,
            search_mode: config
                .enable_embeddings
                .then(|| config.search_mode.clone())
                .flatten(),
            init_attempt_count: AtomicU64::new(0),
            last_init_error: parking_lot::RwLock::new(None),
        });

        Ok(Self {
            session_manager,
            workspace_manager,
            stores,
            index,
            engine,
            reranker,
            accurate_reranker,
            persister,
            config,
            system_metrics,
            content_vectorizer: Arc::new(OnceCell::new()),
            embedding_config_holder,
            project_locks: Default::default(),
            workspace_graphs: Default::default(),
            derived_work,
            changes,
        })
    }

    /// The search index, if the ports have one.
    pub fn index(&self) -> Option<&Arc<dyn SearchIndex>> {
        self.index.as_ref()
    }

    /// The embedding engine, if the ports have one.
    pub fn engine(&self) -> Option<&Arc<dyn EmbeddingEngine>> {
        self.engine.as_ref()
    }

    /// The local reranker the ports carry.
    pub fn reranker(&self) -> Option<&Arc<dyn Reranker>> {
        self.reranker.as_ref()
    }

    /// The `accurate` reranker the ports carry.
    pub fn accurate_reranker(&self) -> Option<&Arc<dyn Reranker>> {
        self.accurate_reranker.as_ref()
    }

    /// Finish the queued embeddings and graph merges, then write every
    /// queued session persist and retry failed ones, waiting at most
    /// `limit` in all (the derived work gets at most three quarters of it);
    /// returns how many sessions could not be written. Call before a
    /// graceful shutdown: background work is otherwise lost with the
    /// process. A record whose embedding did not finish in time is embedded
    /// after the next start (see `vectorize_missing_records`).
    pub(crate) async fn flush_all(&self, limit: std::time::Duration) -> usize {
        let start = std::time::Instant::now();
        // Derived work ends in session persists, written by the flush below.
        if !self.derived_work.wait_idle(limit * 3 / 4).await {
            warn!(
                "{} embedding / graph job(s) still pending at shutdown",
                self.derived_work.pending()
            );
        }
        let unwritten = self
            .persister
            .flush_all(limit.saturating_sub(start.elapsed()))
            .await;
        if unwritten > 0 {
            warn!(
                "{} session(s) could not be persisted before shutdown",
                unwritten
            );
        } else {
            info!("All session changes persisted");
        }
        unwritten
    }
}
