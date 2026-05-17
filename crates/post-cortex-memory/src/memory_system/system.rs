use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
#[cfg(feature = "embeddings")]
use tokio::sync::OnceCell;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::performance::PerformanceMonitor;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_storage::rocksdb_storage::RealRocksDBStorage;
use post_cortex_core::workspace::WorkspaceManager;

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

/// Conversation memory system using actors and channels
pub struct ConversationMemorySystem {
    pub session_manager: SessionManager,
    pub context_processor: IncrementalContextProcessor,
    pub graph_manager: SimpleGraphManager,
    pub workspace_manager: Arc<WorkspaceManager>,
    pub storage_actor: StorageActorHandle,
    pub vector_storage: Arc<dyn post_cortex_storage::traits::VectorStorage>,
    pub config: SystemConfig,
    pub performance_monitor: Arc<PerformanceMonitor>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub system_metrics: Arc<SystemMetrics>,

    #[cfg(feature = "embeddings")]
    pub content_vectorizer: Arc<OnceCell<Arc<crate::content_vectorizer::ContentVectorizer>>>,

    #[cfg(feature = "embeddings")]
    pub semantic_query_engine:
        Arc<OnceCell<Arc<crate::semantic_query_engine::SemanticQueryEngine>>>,

    #[cfg(feature = "embeddings")]
    pub embedding_config_holder: Arc<EmbeddingConfigHolder>,
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
                let endpoint = config.surrealdb_endpoint.as_ref()
                    .ok_or_else(|| "SurrealDB endpoint not configured".to_string())?;

                let storage = post_cortex_storage::surrealdb_storage::SurrealDBStorage::new(
                    endpoint,
                    config.surrealdb_username.as_deref(),
                    config.surrealdb_password.as_deref(),
                    config.surrealdb_namespace.as_deref(),
                    config.surrealdb_database.as_deref(),
                ).await
                .map_err(|e| format!("Failed to initialize SurrealDB: {e}"))?;

                let storage_arc = Arc::new(storage);
                return Self::new_with_trait_storage(
                    storage_arc.clone() as Arc<dyn post_cortex_storage::traits::GraphStorage>,
                    storage_arc as Arc<dyn post_cortex_storage::traits::VectorStorage>,
                    config,
                ).await;
            }
        }

        // Default: use RocksDB
        let storage = RealRocksDBStorage::new(&config.data_directory)
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
        ).await
    }

    /// Create system with trait object storage (supports both RocksDB and SurrealDB)
    async fn new_with_trait_storage(
        storage: Arc<dyn post_cortex_storage::traits::GraphStorage>,
        vector_storage: Arc<dyn post_cortex_storage::traits::VectorStorage>,
        config: SystemConfig,
    ) -> Result<Self, String> {
        let performance_monitor = Arc::new(PerformanceMonitor::new(None));

        // Create storage actor with trait object
        let storage_actor = StorageActor::spawn(storage, Arc::clone(&performance_monitor)).await?;

        let session_manager = SessionManager::new(
            config.cache_capacity,
            storage_actor.clone(),
            Arc::clone(&performance_monitor),
        )?;

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
                    _ => {
                        warn!(
                            "Unknown embedding model type: {}, defaulting to MultilingualMiniLM",
                            config.embeddings_model_type
                        );
                        EmbeddingModelType::MultilingualMiniLM
                    }
                };

                // Store configuration for lazy initialization
                let embedding_config_holder = Arc::new(EmbeddingConfigHolder {
                    model_type,
                    vector_dimension: config.vector_dimension,
                    max_vectors_per_session: config.max_vectors_per_session,
                    data_directory: config.data_directory.clone(),
                    cross_session_search_enabled: config.cross_session_search_enabled,
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
            performance_monitor,
            circuit_breaker,
            system_metrics,
            #[cfg(feature = "embeddings")]
            content_vectorizer,
            #[cfg(feature = "embeddings")]
            semantic_query_engine,
            #[cfg(feature = "embeddings")]
            embedding_config_holder,
        })
    }

    /// Create a new session with name and description
    pub async fn create_session(
        &self,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<Uuid, String> {
        let _timer = self.performance_monitor.start_timer("create_session");
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
        let session_arc = Arc::new(ArcSwap::new(Arc::new(session.clone())));

        // Save to storage first
        match self.storage_actor.save_session(session).await {
            Ok(_) => {
                // Add to cache and active sessions only if storage save succeeded
                self.session_manager
                    .sessions
                    .put(session_id, Arc::clone(&session_arc));

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("System time before UNIX epoch")
                    .as_secs();
                self.session_manager
                    .active_sessions
                    .insert(session_id, Arc::new(AtomicU64::new(now)));
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
        let _timer = self.performance_monitor.start_timer("get_session");

        if let Some(session_arc) = self.session_manager.sessions.get(&session_id) {
            // Update access time
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_secs();
            if let Some(access_time) = self.session_manager.active_sessions.get(&session_id) {
                access_time.store(now, Ordering::Relaxed);
            }
            return Ok(session_arc);
        }

        // Try loading from storage via actor
        match self.storage_actor.load_session(session_id).await {
            Ok(Some(session)) => {
                let session_arc = Arc::new(ArcSwap::new(Arc::new(session)));
                self.session_manager
                    .sessions
                    .put(session_id, Arc::clone(&session_arc));

                // Add to active sessions
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("System time before UNIX epoch")
                    .as_secs();
                self.session_manager
                    .active_sessions
                    .insert(session_id, Arc::new(AtomicU64::new(now)));

                Ok(session_arc)
            }
            Ok(None) => Err(format!("Session {session_id} not found")),
            Err(e) => Err(format!("Storage error: {e}")),
        }
    }

    // MCP Compatibility methods

    /// Get storage actor handle for compatibility (replaces storage.read().await)
    pub fn get_storage(&self) -> &StorageActorHandle {
        &self.storage_actor
    }

    /// Update session metadata compatibility method
    pub async fn update_session_metadata(
        &self,
        session_id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<(), String> {
        let session_arc = self.get_session(session_id).await?;

        // Update session atomically
        let current_session = session_arc.load();
        let mut new_session = (**current_session).clone();

        // Use the update_metadata method to modify metadata
        new_session.update_metadata(name, description);

        session_arc.store(Arc::new(new_session));
        Ok(())
    }

    /// Delete a session: evicts both in-memory caches before tearing down the
    /// persisted row. Without the cache evict step, `get_session` (used by
    /// `session action=load`) would still hand out the cached ActiveSession
    /// after the underlying storage row was already gone.
    pub async fn delete_session(&self, session_id: Uuid) -> Result<bool, String> {
        self.session_manager.sessions.remove(&session_id);
        self.session_manager.active_sessions.remove(&session_id);

        self.storage_actor.delete_session(session_id).await
    }

    /// Delete a single entity from a session: removes the node + incident
    /// edges from the in-memory graph (so cached writes don't resurrect it)
    /// and cascades the same delete to storage (entity row + all 8 edge
    /// tables). Returns whether the entity existed.
    pub async fn delete_entity(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<bool, String> {
        let session_arc = self.get_session(session_id).await?;

        let current = session_arc.load();
        let existed = current.entity_graph.has_entity(entity_name);
        if existed {
            let mut new_session = (**current).clone();
            let graph_mut = std::sync::Arc::make_mut(&mut new_session.entity_graph);
            graph_mut.remove_entity(entity_name);
            session_arc.store(Arc::new(new_session));
        }

        self.storage_actor
            .delete_entity(session_id, entity_name)
            .await?;

        Ok(existed)
    }

    /// Delete a single incremental update (context entry) by its entry_id.
    ///
    /// Used by MCP `manage_entity` action=delete_update to clean up ghost/bad rows
    /// produced when a client posts content the dispatcher can't map to a title.
    /// Persists the updated session via the storage actor. Returns true if the
    /// entry existed.
    pub async fn delete_update(
        &self,
        session_id: Uuid,
        entry_id: Uuid,
    ) -> Result<bool, String> {
        const MAX_CAS_ATTEMPTS: u32 = 20;
        let session_arc = self.get_session(session_id).await?;

        for attempt in 1..=MAX_CAS_ATTEMPTS {
            let current = session_arc.load();
            let mut new_session = (**current).clone();
            let existed = new_session.remove_update_by_id(&entry_id);

            if !existed {
                return Ok(false);
            }

            let new_arc = Arc::new(new_session);
            let prev = session_arc.compare_and_swap(&current, Arc::clone(&new_arc));
            if Arc::ptr_eq(&prev, &current) {
                self.storage_actor
                    .persist_session_and_update_nowait((*new_arc).clone(), vec![]);
                return Ok(true);
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
            "delete_update: high contention ({MAX_CAS_ATTEMPTS} CAS attempts exhausted) for session {session_id}"
        ))
    }

    /// Find sessions by name or description compatibility method
    pub async fn find_sessions_by_name_or_description(
        &self,
        query: &str,
    ) -> Result<Vec<Uuid>, String> {
        let session_ids = self.list_sessions().await?;
        let query_lower = query.to_lowercase();
        let mut matching_sessions = Vec::new();

        for session_id in &session_ids {
            // get_session already does cache-then-storage lookup
            let Ok(session_arc) = self.get_session(*session_id).await else {
                continue;
            };
            let session = session_arc.load();

            let name_match = session
                .name()
                .as_ref()
                .is_some_and(|n| n.to_lowercase().contains(&query_lower));
            let desc_match = session
                .description()
                .as_ref()
                .is_some_and(|d| d.to_lowercase().contains(&query_lower));

            if name_match || desc_match {
                matching_sessions.push(*session_id);
            }
        }

        Ok(matching_sessions)
    }

    /// List all sessions
    pub async fn list_sessions(&self) -> Result<Vec<Uuid>, String> {
        let _timer = self.performance_monitor.start_timer("list_sessions");

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

    /// Add incremental update - compatibility wrapper
    #[instrument(skip(self, session_id, description, metadata))]
    pub async fn add_incremental_update(
        &self,
        session_id: Uuid,
        description: String,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, String> {
        debug!("add_incremental_update START for session {}", session_id);
        let _timer = self
            .performance_monitor
            .start_timer("add_incremental_update");
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
            .add_incremental_update_internal(session_id, description, metadata)
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
    ) -> Result<String, String> {
        // Content size limit to prevent timeouts
        if description.len() > 2000 {
            // Truncate at nearest UTF-8 boundary at or before 1800 bytes
            let mut safe_len = 1800;
            while safe_len > 0 && !description.is_char_boundary(safe_len) {
                safe_len -= 1;
            }
            description.truncate(safe_len);
            description.push_str("... (truncated)");
            debug!("Truncated long description to prevent timeout (UTF-8 safe)");
        }

        // Get or create session
        let session_arc = self
            .session_manager
            .get_or_create_session(session_id)
            .await?;

        // Update session atomically using CAS loop to prevent race conditions
        let update_result = {
            let start = std::time::Instant::now();
            const MAX_CAS_ATTEMPTS: u32 = 20;
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

                // Add context update
                let result = new_session
                    .add_context_update(description.clone(), metadata.clone())
                    .await;

                match result {
                    Ok((uid, update)) => {
                        // Try to swap atomically
                        let new_arc = Arc::new(new_session);
                        let prev_arc = session_arc.compare_and_swap(&current_arc, new_arc);

                        // Check if swap was successful (pointer equality)
                        if Arc::ptr_eq(&prev_arc, &current_arc) {
                            if attempts > 1 {
                                debug!("CAS succeeded after {} attempts for session {}", attempts, session_id);
                            }
                            break Ok((uid, update));
                        }

                        // CAS failed, retry with exponential backoff
                        tracing::debug!("CAS failed for session {}, retrying (attempt {})", session_id, attempts);
                        if attempts > 3 {
                            tokio::time::sleep(std::time::Duration::from_micros(100 * (1 << (attempts - 3).min(5)))).await;
                        } else {
                            tokio::task::yield_now().await;
                        }
                    },
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

                // Fire-and-forget: enqueue combined session + update persistence
                // Data is already committed in-memory via ArcSwap CAS
                let current_session = session_arc.load();
                self.storage_actor.persist_session_and_update_nowait(
                    (**current_session).clone(),
                    vec![context_update.clone()],
                );
                debug!(
                    "Session {} persistence enqueued (non-blocking)",
                    session_id
                );

                // Background entity graph update (non-blocking, non-critical)
                let session_arc_bg = Arc::clone(&session_arc);
                let storage_actor_bg = self.storage_actor.clone();
                let update_for_graph = context_update;
                tokio::spawn(async move {
                    let current = session_arc_bg.load();
                    let mut new_session = (**current).clone();

                    if let Err(e) = new_session
                        .apply_entity_graph_update(&update_for_graph)
                        .await
                    {
                        warn!("Background entity graph update failed: {}", e);
                        return;
                    }

                    // CAS-swap the graph update back and persist only on success.
                    let new_arc = Arc::new(new_session);
                    let prev = session_arc_bg.compare_and_swap(&current, Arc::clone(&new_arc));
                    if Arc::ptr_eq(&prev, &current) {
                        // CAS succeeded — persist session with updated entity graph
                        storage_actor_bg.persist_session_and_update_nowait(
                            (*new_arc).clone(),
                            vec![],
                        );
                    } else {
                        debug!("Background entity graph CAS failed (concurrent update), skipping");
                    }
                });

                #[cfg(feature = "embeddings")]
                self.spawn_background_vectorization(session_id, Arc::clone(&session_arc))
                    .await;

                info!(
                    "Added incremental update to session {}: {} chars",
                    session_id,
                    description.len()
                );
                Ok(update_id)
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
        let _timer = self
            .performance_monitor
            .start_timer("get_conversation_context");
        self.system_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);

        // Try cache first
        if let Some(session_arc) = self.session_manager.sessions.get(&session_id) {
            self.session_manager
                .session_cache_hits
                .fetch_add(1, Ordering::Relaxed);

            // Load session atomically
            let session = session_arc.load();
            let context = session.context_summary();

            // Update access timestamp
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_secs();
            if let Some(access_time) = self.session_manager.active_sessions.get(&session_id) {
                access_time.store(now, Ordering::Relaxed);
            }

            self.system_metrics
                .successful_requests
                .fetch_add(1, Ordering::Relaxed);
            return Ok(context);
        }

        // Cache miss - load from storage via actor
        self.session_manager
            .session_cache_misses
            .fetch_add(1, Ordering::Relaxed);

        match self.storage_actor.load_session(session_id).await {
            Ok(Some(session)) => {
                // Cache the loaded session using ArcSwap
                let session_arc = Arc::new(ArcSwap::new(Arc::new(session)));
                self.session_manager
                    .sessions
                    .put(session_id, Arc::clone(&session_arc));

                // Add to active sessions
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("System time before UNIX epoch")
                    .as_secs();
                self.session_manager
                    .active_sessions
                    .insert(session_id, Arc::new(AtomicU64::new(now)));

                let context = session_arc.load().context_summary();
                self.system_metrics
                    .successful_requests
                    .fetch_add(1, Ordering::Relaxed);
                Ok(context)
            }
            Ok(None) => {
                self.system_metrics
                    .failed_requests
                    .fetch_add(1, Ordering::Relaxed);
                Err("Session not found".to_string())
            }
            Err(e) => {
                self.system_metrics
                    .failed_requests
                    .fetch_add(1, Ordering::Relaxed);
                Err(format!("Storage error: {e}"))
            }
        }
    }

    /// Get system health - all atomic reads
    pub fn get_system_health(&self) -> SystemHealth {
        let _perf_snapshot = self.performance_monitor.get_snapshot();
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
        let _timer = self
            .performance_monitor
            .start_timer("cleanup_expired_sessions");
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
        let expired_count = expired_sessions.len();
        for session_id in expired_sessions {
            self.session_manager.active_sessions.remove(&session_id);
            self.session_manager.sessions.remove(&session_id);
            debug!("Cleaned up expired session: {}", session_id);
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
        let evicted_count = session_times.iter().take(count).count();
        for (session_id, _) in session_times.iter().take(count) {
            self.session_manager.active_sessions.remove(session_id);
            self.session_manager.sessions.remove(session_id);
            debug!("Evicted session {} from cache (memory limit)", session_id);
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
