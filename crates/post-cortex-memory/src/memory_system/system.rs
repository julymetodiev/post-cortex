use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
#[cfg(feature = "embeddings")]
use tokio::sync::OnceCell;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::{ActiveSession, MAX_DESCRIPTION_BYTES};
use post_cortex_core::workspace::WorkspaceManager;
use post_cortex_storage::rocksdb_storage::RealRocksDBStorage;
use post_cortex_storage::traits::SessionSummary;

use super::circuit_breaker::CircuitBreaker;
use super::config::SystemConfig;
use super::managers::{IncrementalContextProcessor, SimpleGraphManager};
use super::metrics::{SystemHealth, SystemMetrics};
use super::session_manager::SessionManager;
use super::storage_actor::{StorageActor, StorageActorHandle};

#[cfg(feature = "embeddings")]
use super::config::EmbeddingConfigHolder;
#[cfg(feature = "embeddings")]
use post_cortex_embeddings::EmbeddingModelType;

// Memory management constants
const MAX_ACTIVE_SESSIONS: usize = 500;
const SESSION_CLEANUP_BATCH_SIZE: usize = 50;
/// Compare-and-swap attempts before a session write gives up.
const MAX_CAS_ATTEMPTS: u32 = 20;

/// Wait before retrying a lost compare-and-swap: yield for the first
/// attempts, then back off exponentially (capped at 3.2 ms).
async fn cas_backoff(attempt: u32) {
    if attempt > 3 {
        tokio::time::sleep(std::time::Duration::from_micros(
            100 * (1 << (attempt - 3).min(5)),
        ))
        .await;
    } else {
        tokio::task::yield_now().await;
    }
}

/// Remove what vectorizing `update_id` added for a session that was
/// deleted meanwhile: its vectors (memory and persisted) and cached results.
#[cfg(feature = "embeddings")]
pub(super) async fn undo_vectorization(
    vectorizer: &crate::content_vectorizer::ContentVectorizer,
    session_id: Uuid,
    update_id: Uuid,
) {
    if let Err(e) = vectorizer
        .remove_content(session_id, &update_id.to_string())
        .await
    {
        warn!(
            "Failed to remove embedding {} of deleted session {}: {:#}",
            update_id, session_id, e
        );
    }
    vectorizer.remove_session_vectors(session_id);
}

/// How a write handles the work derived from a new update.
#[derive(Debug, Clone, Copy)]
enum WriteMode {
    /// Merge the update into the entity graph in the same swap, persist in
    /// the background and vectorize in the background.
    Inline,
    /// Leave graph merge and vectorization to the caller (the pipeline);
    /// optionally wait until the update is written to storage.
    Deferred {
        /// Wait for the storage write before returning.
        wait_durable: bool,
    },
}

/// An update committed by [`ConversationMemorySystem::add_incremental_update_deferred`].
#[derive(Debug, Clone)]
pub struct CommittedUpdate {
    /// Id of the new entry.
    pub entry_id: Uuid,
    /// The update as stored in the session.
    pub update: ContextUpdate,
    /// Whether the update was confirmed written to storage before the call
    /// returned (only ever true when the caller asked to wait for it).
    pub durable: bool,
}

/// Conversation memory system using actors and channels
pub struct ConversationMemorySystem {
    /// Session lifecycle manager with cache
    pub session_manager: SessionManager,
    /// Incremental context processing metrics
    pub context_processor: IncrementalContextProcessor,
    /// Knowledge graph metric container
    pub graph_manager: SimpleGraphManager,
    /// Workspace management coordinator
    pub workspace_manager: Arc<WorkspaceManager>,
    /// Handle to the async storage actor
    pub storage_actor: StorageActorHandle,
    /// Vector storage backend for semantic search
    pub vector_storage: Arc<dyn post_cortex_storage::traits::VectorStorage>,
    /// System configuration
    pub config: SystemConfig,
    /// Circuit breaker for fault tolerance
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Global system metrics
    pub system_metrics: Arc<SystemMetrics>,

    /// Lazily initialized content vectorizer
    #[cfg(feature = "embeddings")]
    pub content_vectorizer: Arc<OnceCell<Arc<crate::content_vectorizer::ContentVectorizer>>>,

    /// Lazily initialized semantic query engine
    #[cfg(feature = "embeddings")]
    pub semantic_query_engine:
        Arc<OnceCell<Arc<crate::semantic_query_engine::SemanticQueryEngine>>>,

    /// Configuration holder used for lazy embedding initialization
    #[cfg(feature = "embeddings")]
    pub embedding_config_holder: Arc<EmbeddingConfigHolder>,

    /// Retrieval graph per session (see `graph_candidates`)
    #[cfg(feature = "embeddings")]
    pub(crate) retrieval_graphs: Arc<super::embeddings::SessionGraphs>,
}

impl ConversationMemorySystem {
    /// Create system from config
    ///
    /// # Errors
    /// Returns an error if storage initialization fails or if system setup encounters any issues
    pub async fn new(config: SystemConfig) -> Result<Self, String> {
        // Check storage backend configuration
        #[cfg(feature = "surrealdb-storage")]
        {
            use post_cortex_storage::traits::StorageBackendType;

            if config.storage_backend == StorageBackendType::SurrealDB {
                let endpoint = config
                    .surrealdb_endpoint
                    .as_ref()
                    .ok_or_else(|| "SurrealDB endpoint not configured".to_string())?;

                let storage =
                    post_cortex_storage::surrealdb_storage::SurrealDBStorage::new_with_dimension(
                        endpoint,
                        config.surrealdb_username.as_deref(),
                        config.surrealdb_password.as_deref(),
                        config.surrealdb_namespace.as_deref(),
                        config.surrealdb_database.as_deref(),
                        config.vector_dimension,
                    )
                    .await
                    .map_err(|e| format!("Failed to initialize SurrealDB: {e}"))?;

                let storage_arc = Arc::new(storage);
                return Self::new_with_trait_storage(
                    storage_arc.clone() as Arc<dyn post_cortex_storage::traits::GraphStorage>,
                    storage_arc as Arc<dyn post_cortex_storage::traits::VectorStorage>,
                    config,
                )
                .await;
            }
        }

        // Default: use RocksDB
        let storage =
            RealRocksDBStorage::new_with_dimension(&config.data_directory, config.vector_dimension)
                .await
                .map_err(|e| format!("Failed to initialize storage: {e}"))?;

        Self::new_with_rocksdb(storage, config).await
    }

    /// Create system with RocksDB storage (backward compatibility)
    pub async fn new_with_storage(
        storage: RealRocksDBStorage,
        config: SystemConfig,
    ) -> Result<Self, String> {
        Self::new_with_rocksdb(storage, config).await
    }

    /// Create system with RocksDB storage
    async fn new_with_rocksdb(
        storage: RealRocksDBStorage,
        config: SystemConfig,
    ) -> Result<Self, String> {
        let storage_arc = Arc::new(storage);
        Self::new_with_trait_storage(
            storage_arc.clone() as Arc<dyn post_cortex_storage::traits::GraphStorage>,
            storage_arc as Arc<dyn post_cortex_storage::traits::VectorStorage>,
            config,
        )
        .await
    }

    /// Create system with trait object storage (supports both RocksDB and SurrealDB)
    // Still copies the deprecated `max_vectors_per_session` into the holder.
    #[allow(deprecated)]
    async fn new_with_trait_storage(
        storage: Arc<dyn post_cortex_storage::traits::GraphStorage>,
        vector_storage: Arc<dyn post_cortex_storage::traits::VectorStorage>,
        config: SystemConfig,
    ) -> Result<Self, String> {
        // Create storage actor with trait object
        let storage_actor = StorageActor::spawn(storage).await?;

        let session_manager = SessionManager::new(config.cache_capacity, storage_actor.clone())?;

        let context_processor = IncrementalContextProcessor {
            contexts_processed: Arc::new(AtomicU64::new(0)),
            processing_errors: Arc::new(AtomicU64::new(0)),
            total_processing_time_ns: Arc::new(AtomicU64::new(0)),
            avg_processing_time_ns: Arc::new(AtomicU64::new(0)),
        };

        let graph_manager = SimpleGraphManager {
            entities_count: Arc::new(AtomicUsize::new(0)),
            relationships_count: Arc::new(AtomicUsize::new(0)),
            graph_operations: Arc::new(AtomicU64::new(0)),
            graph_updates: Arc::new(AtomicU64::new(0)),
        };

        let circuit_breaker = Arc::new(CircuitBreaker::new(
            config.circuit_breaker_failure_threshold,
            config.circuit_breaker_timeout_seconds,
        ));

        let system_metrics = Arc::new(SystemMetrics::new());

        // Create workspace manager
        let workspace_manager = Arc::new(WorkspaceManager::new());

        // Hydrate workspaces from storage
        match storage_actor.list_all_workspaces().await {
            Ok(workspaces) => {
                tracing::info!("Hydrating {} workspaces from storage", workspaces.len());
                for stored_ws in workspaces {
                    workspace_manager.restore_workspace(
                        stored_ws.id,
                        stored_ws.name,
                        stored_ws.description,
                        stored_ws.sessions,
                    );
                }
            }
            Err(e) => {
                // Just log error, don't fail startup as this is optional functionality or might fail on fresh install
                tracing::warn!(
                    "Failed to hydrate workspaces (this is expected on first run): {}",
                    e
                );
            }
        }

        // Prepare embeddings configuration for lazy initialization
        #[cfg(feature = "embeddings")]
        let (content_vectorizer, semantic_query_engine, embedding_config_holder) =
            if config.enable_embeddings {
                info!("Embeddings enabled - will lazy-initialize on first use");

                // Parse embedding model type
                let model_type = match config.embeddings_model_type.as_str() {
                    "StaticSimilarityMRL" => EmbeddingModelType::StaticSimilarityMRL,
                    "MiniLM" => EmbeddingModelType::MiniLM,
                    "MultilingualMiniLM" => EmbeddingModelType::MultilingualMiniLM,
                    "TinyBERT" => EmbeddingModelType::TinyBERT,
                    "BGESmall" => EmbeddingModelType::BGESmall,
                    "PotionMultilingual" => EmbeddingModelType::PotionMultilingual,
                    "PotionCode" => EmbeddingModelType::PotionCode,
                    _ => {
                        warn!(
                            "Unknown embedding model type: {}, defaulting to PotionMultilingual",
                            config.embeddings_model_type
                        );
                        EmbeddingModelType::PotionMultilingual
                    }
                };

                // Store configuration for lazy initialization
                let embedding_config_holder = Arc::new(EmbeddingConfigHolder {
                    model_type,
                    vector_dimension: config.vector_dimension,
                    max_vectors_per_session: config.max_vectors_per_session,
                    data_directory: config.data_directory.clone(),
                    cross_session_search_enabled: config.cross_session_search_enabled,
                    reranker_model: config.reranker_model.clone(),
                    voyage: config
                        .voyage_api_key
                        .clone()
                        .map(|key| (key, config.voyage_model.clone())),
                    search_mode: config.search_mode.clone(),
                    init_attempt_count: AtomicU64::new(0),
                    last_init_error: parking_lot::RwLock::new(None),
                });

                (
                    Arc::new(OnceCell::new()),
                    Arc::new(OnceCell::new()),
                    embedding_config_holder,
                )
            } else {
                info!("Embeddings disabled in configuration");
                (
                    Arc::new(OnceCell::new()),
                    Arc::new(OnceCell::new()),
                    Arc::new(EmbeddingConfigHolder {
                        model_type: EmbeddingModelType::StaticSimilarityMRL,
                        vector_dimension: 1024,
                        max_vectors_per_session: 10000,
                        data_directory: config.data_directory.clone(),
                        cross_session_search_enabled: false,
                        reranker_model: None,
                        voyage: None,
                        search_mode: None,
                        init_attempt_count: AtomicU64::new(0),
                        last_init_error: parking_lot::RwLock::new(None),
                    }),
                )
            };

        #[cfg(not(feature = "embeddings"))]
        if config.enable_embeddings {
            warn!("Embeddings requested but 'embeddings' feature not enabled");
        }

        Ok(Self {
            session_manager,
            context_processor,
            graph_manager,
            workspace_manager,
            storage_actor,
            vector_storage,
            config,
            circuit_breaker,
            system_metrics,
            #[cfg(feature = "embeddings")]
            content_vectorizer,
            #[cfg(feature = "embeddings")]
            semantic_query_engine,
            #[cfg(feature = "embeddings")]
            embedding_config_holder,
            #[cfg(feature = "embeddings")]
            retrieval_graphs: Arc::default(),
        })
    }

    /// Create a new session with name and description
    pub async fn create_session(
        &self,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<Uuid, String> {
        self.system_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);

        let session_id = Uuid::new_v4();

        // Create session with provided name and description
        let session_name = name.or_else(|| {
            Some(format!(
                "Session {}",
                session_id
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("unknown")
            ))
        });

        let session_description =
            description.or_else(|| Some("New conversation session".to_string()));

        let session = ActiveSession::new(session_id, session_name, session_description);

        // Save to storage first
        match self.storage_actor.save_session(session.clone()).await {
            Ok(_) => {
                // Add to cache and active sessions only if storage save succeeded
                self.session_manager.install(session_id, session);
                self.session_manager
                    .session_count
                    .fetch_add(1, Ordering::Relaxed);

                self.system_metrics
                    .successful_requests
                    .fetch_add(1, Ordering::Relaxed);
                info!("Created new session: {}", session_id);

                Ok(session_id)
            }
            Err(e) => {
                self.system_metrics
                    .failed_requests
                    .fetch_add(1, Ordering::Relaxed);
                Err(format!("Failed to save session to storage: {e}"))
            }
        }
    }

    /// Get existing session by ID
    pub async fn get_session(
        &self,
        session_id: Uuid,
    ) -> Result<Arc<ArcSwap<ActiveSession>>, String> {
        self.session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| format!("Session {session_id} not found"))
    }

    // MCP Compatibility methods

    /// Get storage actor handle for compatibility (replaces storage.read().await)
    pub fn get_storage(&self) -> &StorageActorHandle {
        &self.storage_actor
    }

    /// Update session metadata compatibility method. Swaps the change in
    /// with a CAS retry loop (a plain store would drop concurrent writes)
    /// and persists it.
    pub async fn update_session_metadata(
        &self,
        session_id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<(), String> {
        let session_arc = self.get_session(session_id).await?;

        let updated = Self::cas_update_session(
            &session_arc,
            session_id,
            "update_session_metadata",
            |session| {
                session.update_metadata(name.clone(), description.clone());
                true
            },
        )
        .await?;

        if updated.is_some() {
            self.storage_actor
                .persist_session_nowait(&session_arc, vec![]);
        }
        Ok(())
    }

    /// Delete a session: evicts it from the in-memory caches, drops its
    /// queued persists (and makes persists still in flight for the evicted
    /// handle no-ops, so the delete cannot be undone by a late write),
    /// deletes it from storage and removes its in-memory vectors and cached
    /// search results. Without the cache evict step, `get_session` (used by
    /// `session action=load`) would still hand out the cached ActiveSession
    /// after the underlying storage row was already gone.
    pub async fn delete_session(&self, session_id: Uuid) -> Result<bool, String> {
        // Under the loading gate: a load cannot read the session before the
        // delete and cache it after. Every handle loaded so far is retired,
        // so none of them (cached or held by a worker) can write it back.
        let deleted = self
            .session_manager
            .with_load_gate(session_id, async {
                let live = self.session_manager.sessions.remove(&session_id);
                self.session_manager.active_sessions.remove(&session_id);
                self.storage_actor.delete_session(session_id, live).await
            })
            .await?;

        #[cfg(feature = "embeddings")]
        {
            if let Some(vectorizer) = self.content_vectorizer.get() {
                vectorizer.remove_session_vectors(session_id);
            }
            self.retrieval_graphs.remove(&session_id);
        }

        Ok(deleted)
    }

    /// Delete a single entity from a session: removes the node + incident
    /// edges from the in-memory graph (CAS, so concurrent writes neither
    /// lose nor resurrect it), persists the session, cascades the delete to
    /// storage (entity row + all 8 edge tables) and drops the entity's
    /// embedding. Returns whether the entity existed.
    pub async fn delete_entity(&self, session_id: Uuid, entity_name: &str) -> Result<bool, String> {
        let session_arc = self.get_session(session_id).await?;

        let updated =
            Self::cas_update_session(&session_arc, session_id, "delete_entity", |session| {
                if !session.entity_graph.has_entity(entity_name) {
                    return false;
                }
                Arc::make_mut(&mut session.entity_graph).remove_entity(entity_name);
                true
            })
            .await?;
        let existed = updated.is_some();
        if existed {
            self.storage_actor
                .persist_session_nowait(&session_arc, vec![]);
        }

        self.storage_actor
            .delete_entity(session_id, entity_name)
            .await?;

        self.drop_content_vectors(session_id, &[format!("entity:{entity_name}")])
            .await;

        Ok(existed)
    }

    /// Delete a single incremental update (context entry) by its entry_id.
    ///
    /// Used by MCP `manage_entity` action=delete_update to clean up ghost/bad rows
    /// produced when a client posts content the dispatcher can't map to a title.
    /// Persists the updated session via the storage actor and drops the
    /// update's embedding (memory, storage, query cache). Returns true if the
    /// entry existed.
    pub async fn delete_update(&self, session_id: Uuid, entry_id: Uuid) -> Result<bool, String> {
        let session_arc = self.get_session(session_id).await?;

        let updated =
            Self::cas_update_session(&session_arc, session_id, "delete_update", |session| {
                session.remove_update_by_id(&entry_id)
            })
            .await?;
        let Some(new_arc) = updated else {
            return Ok(false);
        };

        // Shared set: drop the id before the snapshot is persisted.
        new_arc.vectorized_update_ids.remove(&entry_id);
        // Named: storage never infers a removal from the snapshot.
        self.storage_actor
            .persist_session_removals_nowait(&session_arc, vec![entry_id]);

        self.drop_content_vectors(session_id, &[entry_id.to_string()])
            .await;

        Ok(true)
    }

    /// Apply `mutate` to a copy of the current session and compare-and-swap
    /// it in, retrying with backoff while other writers win. `mutate`
    /// returns false when there is nothing to change; that ends the loop
    /// with `Ok(None)`. On success returns the installed snapshot.
    async fn cas_update_session(
        session_arc: &ArcSwap<ActiveSession>,
        session_id: Uuid,
        op: &str,
        mut mutate: impl FnMut(&mut ActiveSession) -> bool,
    ) -> Result<Option<Arc<ActiveSession>>, String> {
        const MAX_CAS_ATTEMPTS: u32 = 20;

        for attempt in 1..=MAX_CAS_ATTEMPTS {
            let current = session_arc.load_full();
            let mut new_session = (*current).clone();
            if !mutate(&mut new_session) {
                return Ok(None);
            }

            let new_arc = Arc::new(new_session);
            let prev = session_arc.compare_and_swap(&current, Arc::clone(&new_arc));
            if Arc::ptr_eq(&prev, &current) {
                return Ok(Some(new_arc));
            }

            if attempt > 3 {
                tokio::time::sleep(std::time::Duration::from_micros(
                    100 * (1 << (attempt - 3).min(5)),
                ))
                .await;
            } else {
                tokio::task::yield_now().await;
            }
        }

        Err(format!(
            "{op}: high contention ({MAX_CAS_ATTEMPTS} CAS attempts exhausted) for session {session_id}"
        ))
    }

    /// Find sessions whose name or description contains `query`
    /// (case-insensitive). Reads session metadata only: no session is
    /// loaded or added to the cache.
    pub async fn find_sessions_by_name_or_description(
        &self,
        query: &str,
    ) -> Result<Vec<Uuid>, String> {
        Ok(self
            .find_session_summaries(query)
            .await?
            .into_iter()
            .map(|s| s.id)
            .collect())
    }

    /// [`Self::find_sessions_by_name_or_description`] returning the
    /// matching sessions' metadata.
    pub async fn find_session_summaries(&self, query: &str) -> Result<Vec<SessionSummary>, String> {
        let query_lower = query.to_lowercase();
        let matches = |text: &Option<String>| {
            text.as_ref()
                .is_some_and(|t| t.to_lowercase().contains(&query_lower))
        };
        let mut summaries = self.list_session_summaries().await?;
        summaries.retain(|s| matches(&s.name) || matches(&s.description));
        Ok(summaries)
    }

    /// Metadata of every stored session, ordered by creation time (oldest
    /// first). Served from storage's metadata records, so no session is
    /// loaded or added to the cache; sessions that are cached report their
    /// in-memory state, which may be ahead of the last write.
    pub async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>, String> {
        let mut summaries = self
            .storage_actor
            .list_session_summaries()
            .await
            .map_err(|e| format!("Failed to list sessions: {e}"))?;
        for summary in &mut summaries {
            if let Some(session_arc) = self.session_manager.sessions.get(&summary.id) {
                *summary = SessionSummary::from_session(&session_arc.load());
            }
        }
        summaries.sort_by_key(|s| (s.created_at, s.id));
        Ok(summaries)
    }

    /// List all sessions
    pub async fn list_sessions(&self) -> Result<Vec<Uuid>, String> {
        // Get sessions from storage via actor
        match self.storage_actor.list_sessions().await {
            Ok(session_ids) => {
                self.system_metrics
                    .successful_requests
                    .fetch_add(1, Ordering::Relaxed);
                Ok(session_ids)
            }
            Err(e) => {
                self.system_metrics
                    .failed_requests
                    .fetch_add(1, Ordering::Relaxed);
                Err(format!("Failed to list sessions: {e}"))
            }
        }
    }

    /// Merge an update's entities and relations into its session's entity
    /// graph **now** (no spawn), then persist in the background.
    ///
    /// Used by the [`crate::pipeline`] graph worker for updates written via
    /// [`Self::add_incremental_update_deferred`]. A lost compare-and-swap is
    /// retried on the newer version, so a concurrent write never drops the
    /// merge; each attempt starts from a fresh copy, so the update is
    /// applied exactly once. Skips sessions that no longer exist and
    /// updates that were deleted after being queued. Errors propagate so
    /// the caller — typically a pipeline worker — can log them and continue.
    pub async fn apply_entity_graph_update_now(
        &self,
        session_id: Uuid,
        update: ContextUpdate,
    ) -> Result<(), String> {
        let Some(session_arc) = self.session_manager.get_session(session_id).await? else {
            debug!(
                "apply_entity_graph_update_now: session {} not found, skipping",
                session_id
            );
            return Ok(());
        };

        for attempt in 1..=MAX_CAS_ATTEMPTS {
            let current = session_arc.load_full();
            if !current
                .incremental_updates
                .iter()
                .rev()
                .any(|u| u.id == update.id)
            {
                debug!(
                    "apply_entity_graph_update_now: update {} no longer in session {}, skipping",
                    update.id, session_id
                );
                return Ok(());
            }

            let mut new_session = (*current).clone();
            new_session
                .apply_entity_graph_update(&update)
                .await
                .map_err(|e| format!("apply_entity_graph_update failed: {e}"))?;

            let prev = session_arc.compare_and_swap(&current, Arc::new(new_session));
            if Arc::ptr_eq(&prev, &current) {
                self.storage_actor
                    .persist_session_nowait(&session_arc, vec![]);
                return Ok(());
            }
            cas_backoff(attempt).await;
        }

        Err(format!(
            "apply_entity_graph_update_now: high contention ({MAX_CAS_ATTEMPTS} CAS attempts exhausted) for session {session_id}"
        ))
    }

    /// Vectorize one update of a session **now** (no spawn).
    ///
    /// Equivalent body of [`Self::spawn_background_vectorization`] without
    /// the surrounding `tokio::spawn` — lets the
    /// [`crate::pipeline::EmbeddingQueue`] worker run vectorization from
    /// inside its own task while the write path stays in single-digit ms.
    /// Takes the update id: the queue may run after later writes landed.
    #[cfg(feature = "embeddings")]
    pub async fn vectorize_update_now(
        &self,
        session_id: Uuid,
        update_id: Uuid,
    ) -> Result<usize, String> {
        if !self.config.enable_embeddings || !self.config.auto_vectorize_on_update {
            return Ok(0);
        }

        let Some(session_arc) = self.session_manager.get_session(session_id).await? else {
            debug!(
                "vectorize_update_now: session {} not found, skipping",
                session_id
            );
            return Ok(0);
        };

        let vectorizer = self
            .ensure_vectorizer_initialized()
            .await
            .map_err(|e| format!("vectorizer init: {e}"))?;

        // Owned snapshot: an ArcSwap guard must not live across the
        // embedding await.
        let session = session_arc.load_full();
        match vectorizer.vectorize_update(&session, update_id).await {
            Ok(count) if count > 0 => {
                if self.storage_actor.is_deleted(session_id, &session_arc) {
                    // Deleted while embedding: undo, or the vectors outlive it.
                    undo_vectorization(&vectorizer, session_id, update_id).await;
                    return Ok(0);
                }
                let _ = vectorizer.invalidate_session_cache(session_id).await;
                self.storage_actor
                    .persist_session_nowait(&session_arc, vec![]);
                Ok(count)
            }
            Ok(_) => Ok(0),
            Err(e) => Err(format!("vectorize_update: {e}")),
        }
    }

    /// Add an update to a session (created if it does not exist) and
    /// return its entry id.
    ///
    /// Self-contained: the update is merged into the entity graph in the
    /// same atomic swap, and persistence and vectorization run in the
    /// background. [`crate::MemoryServiceImpl`] uses
    /// [`Self::add_incremental_update_deferred`] instead and hands the
    /// derived work to its pipeline.
    #[instrument(skip(self, session_id, description, metadata))]
    pub async fn add_incremental_update(
        &self,
        session_id: Uuid,
        description: String,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, String> {
        self.add_incremental_update_with(session_id, description, metadata, WriteMode::Inline)
            .await
            .map(|c| c.entry_id.to_string())
    }

    /// Add an update to a session (created if it does not exist) without
    /// its derived work: the caller must merge it into the entity graph
    /// ([`Self::apply_entity_graph_update_now`]) and vectorize it
    /// ([`Self::vectorize_update_now`]), as the pipeline does.
    ///
    /// With `wait_durable`, returns only after the update's storage write
    /// finished; [`CommittedUpdate::durable`] reports whether it succeeded.
    /// A failed write does not fail the call: the update is committed in
    /// memory and later persists of the session retry the write.
    pub async fn add_incremental_update_deferred(
        &self,
        session_id: Uuid,
        description: String,
        metadata: Option<serde_json::Value>,
        wait_durable: bool,
    ) -> Result<CommittedUpdate, String> {
        self.add_incremental_update_with(
            session_id,
            description,
            metadata,
            WriteMode::Deferred { wait_durable },
        )
        .await
    }

    async fn add_incremental_update_with(
        &self,
        session_id: Uuid,
        description: String,
        metadata: Option<serde_json::Value>,
        mode: WriteMode,
    ) -> Result<CommittedUpdate, String> {
        debug!("add_incremental_update START for session {}", session_id);
        self.system_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);
        self.system_metrics
            .active_operations
            .fetch_add(1, Ordering::Relaxed);

        // Circuit breaker check - atomic
        if self.circuit_breaker.is_open() {
            warn!("Circuit breaker is open - rejecting request");
            self.system_metrics
                .active_operations
                .fetch_sub(1, Ordering::Relaxed);
            return Err("System temporarily unavailable - circuit breaker open".to_string());
        }

        let result = self
            .add_incremental_update_internal(session_id, description, metadata, mode)
            .await;

        // Update circuit breaker based on result
        match &result {
            Ok(_) => {
                self.circuit_breaker.record_success();
                self.system_metrics
                    .successful_requests
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.circuit_breaker.record_failure();
                self.system_metrics
                    .failed_requests
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        self.system_metrics
            .active_operations
            .fetch_sub(1, Ordering::Relaxed);
        result
    }

    async fn add_incremental_update_internal(
        &self,
        session_id: Uuid,
        mut description: String,
        metadata: Option<serde_json::Value>,
        mode: WriteMode,
    ) -> Result<CommittedUpdate, String> {
        // Content size limit (see `MAX_DESCRIPTION_BYTES`).
        if description.len() > MAX_DESCRIPTION_BYTES {
            // Truncate at the nearest UTF-8 boundary at or before the limit
            let mut safe_len = MAX_DESCRIPTION_BYTES;
            while safe_len > 0 && !description.is_char_boundary(safe_len) {
                safe_len -= 1;
            }
            description.truncate(safe_len);
            description.push_str("... (truncated)");
            debug!("Truncated description over {} bytes", MAX_DESCRIPTION_BYTES);
        }

        // Get or create session
        let session_arc = self
            .session_manager
            .get_or_create_session(session_id)
            .await?;

        // Update session atomically using CAS loop to prevent race conditions
        let update_result = {
            let start = std::time::Instant::now();
            let mut attempts = 0;

            let result_holder = loop {
                attempts += 1;
                if attempts > MAX_CAS_ATTEMPTS {
                    break Err(format!(
                        "Failed to update session: high contention ({} CAS attempts exhausted)",
                        MAX_CAS_ATTEMPTS
                    ));
                }

                // Load current session and clone for mutation
                let current_arc = session_arc.load();
                let mut new_session = (**current_arc).clone();

                // Add context update (and, inline, its graph merge: one
                // swap, so the merge is neither lost nor applied twice)
                let mut result = new_session
                    .add_context_update(description.clone(), metadata.clone())
                    .await;
                if let (WriteMode::Inline, Ok((_, update))) = (mode, &result)
                    && let Err(e) = new_session.apply_entity_graph_update(update).await
                {
                    result = Err(format!("Entity graph update failed: {e}"));
                }

                match result {
                    Ok((uid, update)) => {
                        // Try to swap atomically
                        let new_arc = Arc::new(new_session);
                        let prev_arc = session_arc.compare_and_swap(&current_arc, new_arc);

                        // Check if swap was successful (pointer equality)
                        if Arc::ptr_eq(&prev_arc, &current_arc) {
                            if attempts > 1 {
                                debug!(
                                    "CAS succeeded after {} attempts for session {}",
                                    attempts, session_id
                                );
                            }
                            break Ok((uid, update));
                        }

                        // CAS failed, retry with exponential backoff
                        tracing::debug!(
                            "CAS failed for session {}, retrying (attempt {})",
                            session_id,
                            attempts
                        );
                        cas_backoff(attempts).await;
                    }
                    Err(e) => {
                        // Logic error, not contention
                        break Err(e);
                    }
                }
            };

            // Update processing metrics if successful
            if result_holder.is_ok() {
                let processing_time_ns = start.elapsed().as_nanos() as u64;
                let total_time = self
                    .context_processor
                    .total_processing_time_ns
                    .fetch_add(processing_time_ns, Ordering::Relaxed)
                    + processing_time_ns;
                let processed_count = self
                    .context_processor
                    .contexts_processed
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;

                // Update cached average
                let avg_time = total_time / processed_count;
                self.context_processor
                    .avg_processing_time_ns
                    .store(avg_time, Ordering::Relaxed);
            }

            result_holder
        };

        match update_result {
            Ok((update_id, context_update)) => {
                // Update graph metrics
                self.graph_manager
                    .graph_operations
                    .fetch_add(1, Ordering::Relaxed);
                self.graph_manager
                    .graph_updates
                    .fetch_add(1, Ordering::Relaxed);

                let entry_id = Uuid::parse_str(&update_id)
                    .map_err(|e| format!("Invalid update id {update_id:?}: {e}"))?;

                // Data is already committed in memory via the CAS; the
                // persister coalesces this with other pending writes.
                let durable = match mode {
                    WriteMode::Deferred { wait_durable: true } => match self
                        .storage_actor
                        .persist_session_and_wait(&session_arc, vec![context_update.clone()])
                        .await
                    {
                        Ok(()) => true,
                        Err(e) => {
                            warn!(
                                "Update {} of session {} not yet persisted: {}",
                                entry_id, session_id, e
                            );
                            false
                        }
                    },
                    _ => {
                        self.storage_actor
                            .persist_session_nowait(&session_arc, vec![context_update.clone()]);
                        false
                    }
                };

                #[cfg(feature = "embeddings")]
                if matches!(mode, WriteMode::Inline) {
                    self.spawn_background_vectorization(
                        session_id,
                        Arc::clone(&session_arc),
                        entry_id,
                    )
                    .await;
                }

                info!(
                    "Added incremental update to session {}: {} chars",
                    session_id,
                    description.len()
                );
                Ok(CommittedUpdate {
                    entry_id,
                    update: context_update,
                    durable,
                })
            }
            Err(e) => {
                self.context_processor
                    .processing_errors
                    .fetch_add(1, Ordering::Relaxed);
                warn!("Failed to add incremental update: {}", e);
                Err(e)
            }
        }
    }

    /// Get conversation context
    pub async fn get_conversation_context(&self, session_id: Uuid) -> Result<String, String> {
        self.system_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);

        let result = match self.session_manager.get_session(session_id).await {
            Ok(Some(session_arc)) => Ok(session_arc.load().context_summary()),
            Ok(None) => Err("Session not found".to_string()),
            Err(e) => Err(format!("Storage error: {e}")),
        };
        let counter = if result.is_ok() {
            &self.system_metrics.successful_requests
        } else {
            &self.system_metrics.failed_requests
        };
        counter.fetch_add(1, Ordering::Relaxed);
        result
    }

    /// Get system health - all atomic reads
    pub fn get_system_health(&self) -> SystemHealth {
        let cache_stats = self.session_manager.sessions.get_stats();
        let circuit_breaker_stats = self.circuit_breaker.get_stats();

        SystemHealth {
            total_requests: self.system_metrics.total_requests.load(Ordering::Relaxed),
            successful_requests: self
                .system_metrics
                .successful_requests
                .load(Ordering::Relaxed),
            failed_requests: self.system_metrics.failed_requests.load(Ordering::Relaxed),
            active_operations: self
                .system_metrics
                .active_operations
                .load(Ordering::Relaxed),
            active_sessions: self.session_manager.active_sessions.len(),
            circuit_breaker_open: circuit_breaker_stats.is_open,
            circuit_breaker_failures: circuit_breaker_stats.failure_count,
            cache_hit_rate: cache_stats.hit_rate,
            contexts_processed: self
                .context_processor
                .contexts_processed
                .load(Ordering::Relaxed),
            processing_errors: self
                .context_processor
                .processing_errors
                .load(Ordering::Relaxed),
            storage_operations: self
                .system_metrics
                .storage_operations
                .load(Ordering::Relaxed),
            uptime_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_secs()
                .saturating_sub(self.system_metrics.start_timestamp.load(Ordering::Relaxed)),
        }
    }

    /// Cleanup expired sessions - background task
    /// Also enforces memory limits on active_sessions DashMap
    pub async fn cleanup_expired_sessions(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time before UNIX epoch")
            .as_secs();
        let timeout_seconds = self.config.session_timeout_minutes * 60;

        let mut expired_sessions = Vec::new();

        // Find expired sessions
        for entry in self.session_manager.active_sessions.iter() {
            let last_access = entry.value().load(Ordering::Relaxed);
            if now.saturating_sub(last_access) > timeout_seconds {
                expired_sessions.push(*entry.key());
            }
        }

        // Remove expired sessions
        let mut expired_count = 0;
        for session_id in expired_sessions {
            if self.evict_session(session_id).await {
                expired_count += 1;
                debug!("Cleaned up expired session: {}", session_id);
            }
        }

        if expired_count > 0 {
            info!("Cleaned up {} expired sessions", expired_count);
        }

        // Enforce memory limits - evict oldest sessions if over MAX_ACTIVE_SESSIONS
        let current_size = self.session_manager.active_sessions.len();
        if current_size > MAX_ACTIVE_SESSIONS {
            let to_evict = current_size - MAX_ACTIVE_SESSIONS + SESSION_CLEANUP_BATCH_SIZE;
            self.evict_oldest_sessions(to_evict.min(SESSION_CLEANUP_BATCH_SIZE))
                .await;
        }
    }

    /// Evict oldest sessions from cache to prevent unbounded memory growth
    /// Sessions are NOT deleted from storage, only removed from memory cache
    async fn evict_oldest_sessions(&self, count: usize) {
        if count == 0 {
            return;
        }

        // Collect sessions with their last access times
        let mut session_times: Vec<(Uuid, u64)> = self
            .session_manager
            .active_sessions
            .iter()
            .map(|entry| (*entry.key(), entry.value().load(Ordering::Relaxed)))
            .collect();

        // Sort by last access time (oldest first)
        session_times.sort_by_key(|(_, time)| *time);

        // Evict the oldest sessions
        let mut evicted_count = 0;
        for (session_id, _) in session_times.iter().take(count) {
            if self.evict_session(*session_id).await {
                evicted_count += 1;
                debug!("Evicted session {} from cache (memory limit)", session_id);
            }
        }

        if evicted_count > 0 {
            info!(
                "Evicted {} oldest sessions from cache to enforce memory limit ({}/{})",
                evicted_count,
                self.session_manager.active_sessions.len(),
                MAX_ACTIVE_SESSIONS
            );
        }
    }

    /// Drop a session from the in-memory caches unless that could lose
    /// committed data: queued writes and a failed last write are flushed
    /// first, and the session stays cached when that fails. Returns whether
    /// it was evicted.
    async fn evict_session(&self, session_id: Uuid) -> bool {
        if self.storage_actor.has_unflushed(session_id) {
            match self.session_manager.sessions.get(&session_id) {
                Some(handle) => {
                    if let Err(e) = self
                        .storage_actor
                        .persist_session_and_wait(&handle, vec![])
                        .await
                    {
                        warn!(
                            "Keeping session {} in memory: its changes are not persisted ({})",
                            session_id, e
                        );
                        return false;
                    }
                }
                None => self.storage_actor.wait_persisted(session_id).await,
            }
        }
        self.session_manager.active_sessions.remove(&session_id);
        self.session_manager.sessions.remove(&session_id);
        true
    }

    /// Write every queued session persist and retry failed ones, waiting at
    /// most `limit`; returns how many sessions could not be written. Call
    /// before a graceful shutdown: background persists are otherwise lost
    /// with the process.
    pub async fn flush_all(&self, limit: std::time::Duration) -> usize {
        let unwritten = self.storage_actor.flush_all(limit).await;
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

    /// Force cleanup to reduce memory usage immediately
    /// Useful when the system is under memory pressure
    pub async fn force_memory_cleanup(&self, target_size: usize) {
        let current_size = self.session_manager.active_sessions.len();
        if current_size > target_size {
            let to_evict = current_size - target_size;
            info!(
                "Force cleanup: evicting {} sessions (current: {}, target: {})",
                to_evict, current_size, target_size
            );
            self.evict_oldest_sessions(to_evict).await;
        }
    }

    /// Save workspace metadata (proxy to storage actor)
    pub async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<(), String> {
        self.storage_actor
            .save_workspace_metadata(workspace_id, name, description, session_ids)
            .await
    }
}
