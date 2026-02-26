// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
use crate::core::cache::{Cache, SessionCache};
use crate::core::performance::PerformanceMonitor;
use crate::session::active_session::ActiveSession;
use crate::storage::rocksdb_storage::RealRocksDBStorage;
use crate::workspace::WorkspaceManager;
use anyhow;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{Sender, UnboundedReceiver, UnboundedSender, channel, unbounded_channel};
use tokio::sync::{OnceCell, Semaphore, oneshot};

use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

// Retry configuration constants
const MAX_VECTORIZATION_RETRIES: u32 = 3;
const VECTORIZATION_RETRY_DELAY_MS: u64 = 100;
const MAX_VECTORIZER_INIT_RETRIES: u32 = 2;

// Timeout constants for different operation types
const TIMEOUT_FAST_MS: u64 = 5_000; // 5 seconds for simple operations
const TIMEOUT_MEDIUM_MS: u64 = 30_000; // 30 seconds for normal operations
const TIMEOUT_SLOW_MS: u64 = 120_000; // 2 minutes for bulk operations
const TIMEOUT_VECTORIZATION_MS: u64 = 300_000; // 5 minutes for vectorization

// Memory management constants
const MAX_ACTIVE_SESSIONS: usize = 500; // Maximum sessions in active_sessions DashMap
const SESSION_CLEANUP_BATCH_SIZE: usize = 50; // Sessions to clean per batch

// Parallel processing constants
const MAX_PARALLEL_VECTORIZATION: usize = 4; // Maximum concurrent vectorization tasks

// Conditional imports for embeddings feature
#[cfg(feature = "embeddings")]
use crate::core::content_vectorizer::{ContentVectorizer, ContentVectorizerConfig};
#[cfg(feature = "embeddings")]
use crate::core::embeddings::{EmbeddingConfig, EmbeddingModelType};
#[cfg(feature = "embeddings")]
use crate::core::vector_db::VectorDbConfig;

/// Configuration holder for lazy embedding initialization
#[cfg(feature = "embeddings")]
pub struct EmbeddingConfigHolder {
    pub model_type: EmbeddingModelType,
    pub vector_dimension: usize,
    pub max_vectors_per_session: usize,
    pub data_directory: String,
    pub cross_session_search_enabled: bool,
    /// Tracks initialization attempts for retry mechanism
    pub init_attempt_count: AtomicU64,
    /// Last initialization error (for diagnostics)
    pub last_init_error: parking_lot::RwLock<Option<String>>,
}

/// Conversation memory system using actors and channels
pub struct ConversationMemorySystem {
    /// Session management
    pub session_manager: SessionManager,

    /// Context processing
    pub context_processor: IncrementalContextProcessor,

    /// Graph management
    pub graph_manager: SimpleGraphManager,

    /// Workspace management
    pub workspace_manager: Arc<WorkspaceManager>,

    /// Storage actor handle
    pub storage_actor: StorageActorHandle,

    /// Direct storage reference for VectorStorage trait
    pub vector_storage: Arc<dyn crate::storage::traits::VectorStorage>,

    /// System configuration - immutable after creation
    pub config: SystemConfig,

    /// Performance monitoring
    pub performance_monitor: Arc<PerformanceMonitor>,

    /// Circuit breaker state - all atomic
    pub circuit_breaker: Arc<CircuitBreaker>,

    /// Global system metrics - atomic
    pub system_metrics: Arc<SystemMetrics>,

    /// Embeddings and semantic search components (optional, lazy-initialized)
    #[cfg(feature = "embeddings")]
    pub content_vectorizer: Arc<OnceCell<Arc<crate::core::content_vectorizer::ContentVectorizer>>>,

    #[cfg(feature = "embeddings")]
    pub semantic_query_engine:
        Arc<OnceCell<Arc<crate::core::semantic_query_engine::SemanticQueryEngine>>>,

    /// Embedding configuration for lazy initialization
    #[cfg(feature = "embeddings")]
    pub embedding_config_holder: Arc<EmbeddingConfigHolder>,
}

/// Session manager using `DashMap` and atomic operations
pub struct SessionManager {
    /// Session cache
    pub sessions: SessionCache<Uuid, Arc<ArcSwap<ActiveSession>>>,

    /// Storage communication
    storage_actor: StorageActorHandle,

    /// Configuration
    #[allow(dead_code)]
    config: SystemConfig,

    /// Performance monitoring
    performance_monitor: Arc<PerformanceMonitor>,

    /// Session metrics - all atomic
    pub session_count: Arc<AtomicUsize>,
    pub total_session_operations: Arc<AtomicU64>,
    pub session_cache_hits: Arc<AtomicU64>,
    pub session_cache_misses: Arc<AtomicU64>,
    pub active_sessions: DashMap<Uuid, Arc<AtomicU64>>, // last_access_timestamp
}

/// Incremental context processor
pub struct IncrementalContextProcessor {
    #[allow(dead_code)]
    config: SystemConfig,
    #[allow(dead_code)]
    performance_monitor: Arc<PerformanceMonitor>,

    /// Processing metrics - atomic
    pub contexts_processed: Arc<AtomicU64>,
    pub processing_errors: Arc<AtomicU64>,
    pub total_processing_time_ns: Arc<AtomicU64>,
    pub avg_processing_time_ns: Arc<AtomicU64>,
}

/// Graph manager
pub struct SimpleGraphManager {
    #[allow(dead_code)]
    config: SystemConfig,
    #[allow(dead_code)]
    performance_monitor: Arc<PerformanceMonitor>,

    /// Graph metrics - atomic
    pub entities_count: Arc<AtomicUsize>,
    pub relationships_count: Arc<AtomicUsize>,
    pub graph_operations: Arc<AtomicU64>,
    pub graph_updates: Arc<AtomicU64>,
}

/// Circuit breaker using atomics
#[derive(Debug)]
pub struct CircuitBreaker {
    pub is_open: AtomicBool,
    pub failure_count: AtomicU64,
    pub last_failure_timestamp: AtomicU64,
    pub success_count: AtomicU64,
    pub last_success_timestamp: AtomicU64,

    // Configuration
    pub failure_threshold: u64,
    pub timeout_seconds: u64,
}

/// Global system metrics - all atomic
#[derive(Debug)]
pub struct SystemMetrics {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub active_operations: AtomicUsize,
    pub storage_operations: AtomicU64,
    pub cache_operations: AtomicU64,
    pub start_timestamp: AtomicU64,
}

/// Storage actor for handling all storage operations asynchronously
pub struct StorageActor {
    storage: Arc<dyn crate::storage::traits::Storage>,
    receiver: UnboundedReceiver<StorageMessage>,
    performance_monitor: Arc<PerformanceMonitor>,
    operation_count: AtomicU64,
    load_count: AtomicU64,
    save_count: AtomicU64,
    delete_count: AtomicU64,
}

/// Operation types for dynamic timeout configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    /// Fast operations: get, simple reads (5s)
    Fast,
    /// Medium operations: save, update (30s)
    Medium,
    /// Slow operations: bulk saves, list all (2min)
    Slow,
    /// Vectorization operations: embedding generation (5min)
    Vectorization,
}

impl OperationType {
    /// Get the timeout duration for this operation type
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(TIMEOUT_FAST_MS),
            Self::Medium => Duration::from_millis(TIMEOUT_MEDIUM_MS),
            Self::Slow => Duration::from_millis(TIMEOUT_SLOW_MS),
            Self::Vectorization => Duration::from_millis(TIMEOUT_VECTORIZATION_MS),
        }
    }
}

/// Handle for communicating with storage actor
#[derive(Clone)]
pub struct StorageActorHandle {
    sender: UnboundedSender<StorageMessage>,
}

/// Messages for storage actor
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum StorageMessage {
    LoadSession {
        session_id: Uuid,
        response_tx: Sender<Result<Option<ActiveSession>, String>>,
    },
    SaveSession {
        session: Box<ActiveSession>,
        response_tx: Sender<Result<(), String>>,
    },
    DeleteSession {
        session_id: Uuid,
        response_tx: Sender<Result<bool, String>>,
    },
    ListSessions {
        response_tx: Sender<Result<Vec<Uuid>, String>>,
    },
    GetStats {
        response_tx: Sender<StorageStats>,
    },
    SaveCheckpoint {
        checkpoint: crate::storage::rocksdb_storage::SessionCheckpoint,
        response_tx: Sender<Result<(), String>>,
    },
    LoadCheckpoint {
        checkpoint_id: Uuid,
        response_tx: Sender<Result<crate::storage::rocksdb_storage::SessionCheckpoint, String>>,
    },
    SaveWorkspaceMetadata {
        workspace_id: Uuid,
        name: String,
        description: String,
        session_ids: Vec<Uuid>,
        response_tx: Sender<Result<(), String>>,
    },
    DeleteWorkspace {
        workspace_id: Uuid,
        response_tx: Sender<Result<(), String>>,
    },
    AddSessionToWorkspace {
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
        response_tx: Sender<Result<(), String>>,
    },
    RemoveSessionFromWorkspace {
        workspace_id: Uuid,
        session_id: Uuid,
        response_tx: Sender<Result<(), String>>,
    },
    ListAllWorkspaces {
        response_tx: Sender<Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String>>,
    },
    BatchSaveUpdates {
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
        response_tx: Sender<Result<(), String>>,
    },
    /// Fire-and-forget: persist session + updates without blocking caller.
    /// Actor logs errors but sends no response.
    PersistSessionAndUpdate {
        session: Box<ActiveSession>,
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
    },
    FindRelatedEntities {
        session_id: Uuid,
        entity_name: String,
        response_tx: Sender<Result<Vec<String>, String>>,
    },
    FindShortestPath {
        session_id: Uuid,
        from_entity: String,
        to_entity: String,
        response_tx: Sender<Result<Option<Vec<String>>, String>>,
    },
    Shutdown,
}

/// Storage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub total_operations: u64,
    pub load_operations: u64,
    pub save_operations: u64,
    pub delete_operations: u64,
    pub avg_operation_time_ns: u64,
    pub last_operation_timestamp: u64,
}

/// System configuration
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct SystemConfig {
    pub max_hot_context_size: usize,
    pub max_warm_context_size: usize,
    pub context_compression_threshold: usize,
    pub session_timeout_minutes: u64,
    pub storage_timeout_seconds: u64,
    pub cache_capacity: usize,
    pub enable_performance_monitoring: bool,
    pub circuit_breaker_failure_threshold: u64,
    pub circuit_breaker_timeout_seconds: u64,
    pub data_directory: String,
    // Entity extraction configuration
    pub max_extracted_entities: usize,
    pub max_referenced_entities: usize,
    pub enable_smart_entity_ranking: bool,
    // Embeddings and vectorization configuration
    pub enable_embeddings: bool,
    pub embeddings_model_type: String,
    pub vector_dimension: usize,
    pub max_vectors_per_session: usize,
    pub semantic_search_threshold: f32,
    pub auto_vectorize_on_update: bool,
    pub cross_session_search_enabled: bool,
    // Storage backend configuration
    #[cfg(feature = "surrealdb-storage")]
    pub storage_backend: crate::storage::traits::StorageBackendType,
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_endpoint: Option<String>,
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_username: Option<String>,
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_password: Option<String>,
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_namespace: Option<String>,
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_database: Option<String>,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            max_hot_context_size: 50,
            max_warm_context_size: 200,
            context_compression_threshold: 1000,
            session_timeout_minutes: 30,
            storage_timeout_seconds: 10,
            cache_capacity: 1000,
            enable_performance_monitoring: true,
            circuit_breaker_failure_threshold: 5,
            circuit_breaker_timeout_seconds: 300, // 5 minutes
            data_directory: "./post_cortex_data".to_string(),
            // Entity extraction defaults
            max_extracted_entities: 15,
            max_referenced_entities: 15,
            enable_smart_entity_ranking: true, // Enable by default for better quality
            // Embeddings defaults
            enable_embeddings: true, // Enabled for semantic search functionality
            embeddings_model_type: "MultilingualMiniLM".to_string(),
            vector_dimension: 384, // MultilingualMiniLM uses 384-dimensional embeddings
            max_vectors_per_session: 1000,
            semantic_search_threshold: 0.7,
            auto_vectorize_on_update: true,
            cross_session_search_enabled: true,
            // Storage backend defaults
            #[cfg(feature = "surrealdb-storage")]
            storage_backend: crate::storage::traits::StorageBackendType::RocksDB,
            #[cfg(feature = "surrealdb-storage")]
            surrealdb_endpoint: None,
            #[cfg(feature = "surrealdb-storage")]
            surrealdb_username: None,
            #[cfg(feature = "surrealdb-storage")]
            surrealdb_password: None,
            #[cfg(feature = "surrealdb-storage")]
            surrealdb_namespace: None,
            #[cfg(feature = "surrealdb-storage")]
            surrealdb_database: None,
        }
    }
}

/// Safely truncates a string at the nearest UTF-8 character boundary before `max_bytes`.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_bytes` - Maximum byte length (not character length)
///
/// # Returns
/// The byte index for safe truncation, guaranteed to be at a UTF-8 character boundary
///
/// # Example
/// ```
/// let text = "Hello мир"; // "мир" is Cyrillic, multi-byte UTF-8
/// let safe_len = safe_truncate_len(text, 8);
/// let truncated = &text[..safe_len]; // Won't panic
/// ```
fn safe_truncate_len(s: &str, max_bytes: usize) -> usize {
    if s.len() <= max_bytes {
        return s.len();
    }

    // Find the nearest character boundary at or before max_bytes
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
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
            use crate::storage::traits::StorageBackendType;

            if config.storage_backend == StorageBackendType::SurrealDB {
                let endpoint = config.surrealdb_endpoint.as_ref()
                    .ok_or_else(|| "SurrealDB endpoint not configured".to_string())?;

                let storage = crate::storage::surrealdb_storage::SurrealDBStorage::new(
                    endpoint,
                    config.surrealdb_username.as_deref(),
                    config.surrealdb_password.as_deref(),
                    config.surrealdb_namespace.as_deref(),
                    config.surrealdb_database.as_deref(),
                ).await
                .map_err(|e| format!("Failed to initialize SurrealDB: {e}"))?;

                let storage_arc = Arc::new(storage);
                return Self::new_with_trait_storage(
                    storage_arc.clone() as Arc<dyn crate::storage::traits::Storage>,
                    storage_arc as Arc<dyn crate::storage::traits::VectorStorage>,
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
            storage_arc.clone() as Arc<dyn crate::storage::traits::Storage>,
            storage_arc as Arc<dyn crate::storage::traits::VectorStorage>,
            config,
        ).await
    }

    /// Create system with trait object storage (supports both RocksDB and SurrealDB)
    async fn new_with_trait_storage(
        storage: Arc<dyn crate::storage::traits::Storage>,
        vector_storage: Arc<dyn crate::storage::traits::VectorStorage>,
        config: SystemConfig,
    ) -> Result<Self, String> {
        let performance_monitor = Arc::new(PerformanceMonitor::new(None));

        // Create storage actor with trait object
        let storage_actor = StorageActor::spawn(storage, Arc::clone(&performance_monitor)).await?;

        // Create session cache
        let session_cache = Cache::new(config.cache_capacity, "session_cache".to_string())
            .map_err(|e| format!("Failed to create session cache: {e}"))?;

        let session_manager = SessionManager {
            sessions: session_cache,
            storage_actor: storage_actor.clone(),
            config: config.clone(),
            performance_monitor: Arc::clone(&performance_monitor),
            session_count: Arc::new(AtomicUsize::new(0)),
            total_session_operations: Arc::new(AtomicU64::new(0)),
            session_cache_hits: Arc::new(AtomicU64::new(0)),
            session_cache_misses: Arc::new(AtomicU64::new(0)),
            active_sessions: DashMap::new(),
        };

        let context_processor = IncrementalContextProcessor {
            config: config.clone(),
            performance_monitor: Arc::clone(&performance_monitor),
            contexts_processed: Arc::new(AtomicU64::new(0)),
            processing_errors: Arc::new(AtomicU64::new(0)),
            total_processing_time_ns: Arc::new(AtomicU64::new(0)),
            avg_processing_time_ns: Arc::new(AtomicU64::new(0)),
        };

        let graph_manager = SimpleGraphManager {
            config: config.clone(),
            performance_monitor: Arc::clone(&performance_monitor),
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
        let (_content_vectorizer, _semantic_query_engine) = {
            if config.enable_embeddings {
                warn!("Embeddings requested but 'embeddings' feature not enabled");
            }
        };

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

    /// Find sessions by name or description compatibility method
    pub async fn find_sessions_by_name_or_description(
        &self,
        query: &str,
    ) -> Result<Vec<Uuid>, String> {
        let session_ids = self.list_sessions().await?;
        let query_lower = query.to_lowercase();
        let mut matching_sessions = Vec::new();

        for session_id in &session_ids {
            // Try in-memory cache first, then fall back to storage
            let session_arc = if let Some(cached) = self.session_manager.sessions.get(session_id) {
                cached
            } else if let Ok(loaded) = self.get_session(*session_id).await {
                loaded
            } else {
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
        tracing::info!("add_incremental_update START for session {}", session_id);
        let _timer = self
            .performance_monitor
            .start_timer("add_incremental_update");
        self.system_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);
        self.system_metrics
            .active_operations
            .fetch_add(1, Ordering::Relaxed);

        tracing::info!("Metrics updated, checking circuit breaker...");

        // Circuit breaker check - atomic
        if self.circuit_breaker.is_open() {
            warn!("Circuit breaker is open - rejecting request");
            self.system_metrics
                .active_operations
                .fetch_sub(1, Ordering::Relaxed);
            return Err("System temporarily unavailable - circuit breaker open".to_string());
        }

        tracing::info!("Calling internal implementation...");
        let result = self
            .add_incremental_update_internal(session_id, description, metadata)
            .await;
        tracing::info!("Internal implementation returned: {:?}", result.is_ok());

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
        tracing::info!("Internal: Processing session {}", session_id);
        // Content size limit to prevent timeouts
        if description.len() > 2000 {
            let safe_len = safe_truncate_len(&description, 1800);
            description.truncate(safe_len);
            description.push_str("... (truncated)");
            debug!("Truncated long description to prevent timeout (UTF-8 safe)");
        }

        tracing::info!("Internal: Getting or creating session...");
        // Get or create session
        let session_arc = self
            .session_manager
            .get_or_create_session(session_id)
            .await?;
        tracing::info!("Internal: Session obtained successfully");

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
                let result = Self::add_context_update_to_session(
                    &mut new_session,
                    description.clone(),
                    metadata.clone(),
                ).await;

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

                    // CAS-swap the graph update back. If another update raced,
                    // this CAS fails silently — the next update will catch up.
                    let new_arc = Arc::new(new_session);
                    let prev = session_arc_bg.compare_and_swap(&current, new_arc);
                    if !Arc::ptr_eq(&prev, &current) {
                        debug!("Background entity graph CAS failed (concurrent update), skipping");
                    }
                });

                // Auto-vectorize if embeddings are enabled
                #[cfg(feature = "embeddings")]
                {
                    if let Err(e) = self.auto_vectorize_if_enabled(session_id).await {
                        debug!(
                            "Auto-vectorization warning for session {}: {}",
                            session_id, e
                        );
                    }
                }

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
            let context = Self::get_session_context_summary(&session);

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

                let context = Self::get_session_context_summary(&session_arc.load());
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
}

impl SessionManager {
    /// Get or create session via actor communication
    pub async fn get_or_create_session(
        &self,
        session_id: Uuid,
    ) -> Result<Arc<ArcSwap<ActiveSession>>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("get_or_create_session");
        self.total_session_operations
            .fetch_add(1, Ordering::Relaxed);

        // Try cache first
        if let Some(session_arc) = self.sessions.get(&session_id) {
            self.session_cache_hits.fetch_add(1, Ordering::Relaxed);

            // Update access time
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if let Some(access_time) = self.active_sessions.get(&session_id) {
                access_time.store(now, Ordering::Relaxed);
            }

            return Ok(session_arc);
        }

        // Cache miss

        self.session_cache_misses.fetch_add(1, Ordering::Relaxed);

        // Try loading from storage via actor
        // IMPORTANT: We now differentiate between "not found" and actual storage errors
        // to prevent data loss from temporary storage issues
        let storage_result: Result<Option<ActiveSession>, String> = match self
            .storage_actor
            .load_session(session_id)
            .await
        {
            Ok(Some(session)) => Ok(Some(session)),
            Ok(None) => Ok(None), // Session genuinely not found
            Err(e) => {
                // Check if this is a transient error or a permanent "not found"
                let error_lower = e.to_lowercase();
                if error_lower.contains("not found")
                    || error_lower.contains("does not exist")
                    || error_lower.contains("no such")
                {
                    // This is a "not found" error, treat as Ok(None)
                    debug!("Session {} not found in storage: {}", session_id, e);
                    Ok(None)
                } else if error_lower.contains("timeout") {
                    // Timeout - could be transient, log warning and propagate error
                    warn!(
                        "Timeout loading session {} from storage: {}. Will create new session.",
                        session_id, e
                    );
                    // For get_or_create, we create a new session on timeout
                    // The old session data is preserved in storage and can be recovered
                    Ok(None)
                } else {
                    // Actual storage error - propagate it to prevent silent data loss
                    error!(
                        "Storage error loading session {}: {}. Propagating error instead of masking.",
                        session_id, e
                    );
                    Err(format!("Storage error: {}", e))
                }
            }
        };

        match storage_result {
            Ok(Some(session)) => {
                let session_arc = Arc::new(ArcSwap::new(Arc::new(session)));
                self.sessions.put(session_id, Arc::clone(&session_arc));

                // Add to active sessions
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.active_sessions
                    .insert(session_id, Arc::new(AtomicU64::new(now)));

                Ok(session_arc)
            }
            Ok(None) => {
                // Session not found - create new one
                tracing::info!(
                    "Creating new session {} since not found in storage",
                    session_id
                );
                let new_session = ActiveSession::new(session_id, None, None);
                let session_arc = Arc::new(ArcSwap::new(Arc::new(new_session.clone())));

                // Save to storage first, then cache it
                match self.storage_actor.save_session(new_session).await {
                    Ok(_) => {
                        // Cache it only if storage save succeeded
                        self.sessions.put(session_id, Arc::clone(&session_arc));

                        // Add to active sessions
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        self.active_sessions
                            .insert(session_id, Arc::new(AtomicU64::new(now)));

                        self.session_count.fetch_add(1, Ordering::Relaxed);
                        Ok(session_arc)
                    }
                    Err(e) => Err(format!("Failed to save new session to storage: {e}")),
                }
            }
            Err(e) => Err(format!("Storage error: {e}")),
        }
    }

    /// Get session metrics - all atomic reads
    pub fn get_metrics(&self) -> SessionManagerMetrics {
        let cache_stats = self.sessions.get_stats();

        SessionManagerMetrics {
            session_count: self.session_count.load(Ordering::Relaxed),
            active_sessions: self.active_sessions.len(),
            total_operations: self.total_session_operations.load(Ordering::Relaxed),
            cache_hits: self.session_cache_hits.load(Ordering::Relaxed),
            cache_misses: self.session_cache_misses.load(Ordering::Relaxed),
            cache_hit_rate: cache_stats.hit_rate,
            cache_size: cache_stats.current_size,
            cache_capacity: cache_stats.capacity,
        }
    }
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u64, timeout_seconds: u64) -> Self {
        Self {
            is_open: AtomicBool::new(false),
            failure_count: AtomicU64::new(0),
            last_failure_timestamp: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            last_success_timestamp: AtomicU64::new(0),
            failure_threshold,
            timeout_seconds,
        }
    }

    pub fn is_open(&self) -> bool {
        if !self.is_open.load(Ordering::Relaxed) {
            return false;
        }

        // Check if timeout has passed
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let last_failure = self.last_failure_timestamp.load(Ordering::Relaxed);

        if now.saturating_sub(last_failure) > self.timeout_seconds {
            // Reset circuit breaker
            self.is_open.store(false, Ordering::Relaxed);
            self.failure_count.store(0, Ordering::Relaxed);
            debug!("Circuit breaker reset after timeout");
            false
        } else {
            true
        }
    }

    pub fn record_failure(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let failures = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        self.last_failure_timestamp.store(now, Ordering::Relaxed);

        if failures >= self.failure_threshold {
            self.is_open.store(true, Ordering::Relaxed);
            warn!("Circuit breaker opened after {} failures", failures);
        }
    }

    pub fn record_success(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.success_count.fetch_add(1, Ordering::Relaxed);
        self.last_success_timestamp.store(now, Ordering::Relaxed);

        // Gradually reduce failure count on success
        let current_failures = self.failure_count.load(Ordering::Relaxed);
        if current_failures > 0 {
            self.failure_count.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub fn get_stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            is_open: self.is_open.load(Ordering::Relaxed),
            failure_count: self.failure_count.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
            last_failure_timestamp: self.last_failure_timestamp.load(Ordering::Relaxed),
            last_success_timestamp: self.last_success_timestamp.load(Ordering::Relaxed),
        }
    }
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMetrics {
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            active_operations: AtomicUsize::new(0),
            storage_operations: AtomicU64::new(0),
            cache_operations: AtomicU64::new(0),
            start_timestamp: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            ),
        }
    }
}

impl StorageActorHandle {
    /// Execute an operation with the specified timeout type
    async fn execute_with_timeout<T, F>(
        &self,
        op_type: OperationType,
        op_name: &str,
        future: F,
    ) -> Result<T, String>
    where
        F: std::future::Future<Output = Option<Result<T, String>>>,
    {
        let timeout = op_type.timeout();
        debug!(
            "StorageHandle: {} with {:?} timeout ({}s)",
            op_name,
            op_type,
            timeout.as_secs()
        );

        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| format!("{} timed out after {}s", op_name, timeout.as_secs()))?
            .ok_or_else(|| "Storage actor response channel closed".to_string())?
    }

    pub async fn load_session(&self, session_id: Uuid) -> Result<Option<ActiveSession>, String> {
        let (response_tx, mut response_rx) = channel::<Result<Option<ActiveSession>, String>>(1);

        self.sender
            .send(StorageMessage::LoadSession {
                session_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            &format!("LoadSession {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn save_session(&self, session: ActiveSession) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);
        let session_id = session.id();

        self.sender
            .send(StorageMessage::SaveSession {
                session: Box::new(session),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("SaveSession {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    /// Enqueue session + updates persistence without blocking.
    /// Returns immediately after sending the message. Errors are logged by the actor.
    pub fn persist_session_and_update_nowait(
        &self,
        session: ActiveSession,
        updates: Vec<crate::core::context_update::ContextUpdate>,
    ) {
        let session_id = session.id();
        if let Err(e) = self.sender.send(StorageMessage::PersistSessionAndUpdate {
            session: Box::new(session),
            session_id,
            updates,
        }) {
            warn!(
                "Failed to enqueue background persist for session {}: {}",
                session_id, e
            );
        }
    }

    pub async fn delete_session(&self, session_id: Uuid) -> Result<bool, String> {
        let (response_tx, mut response_rx) = channel::<Result<bool, String>>(1);

        self.sender
            .send(StorageMessage::DeleteSession {
                session_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("DeleteSession {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn list_sessions(&self) -> Result<Vec<Uuid>, String> {
        let (response_tx, mut response_rx) = channel::<Result<Vec<Uuid>, String>>(1);

        self.sender
            .send(StorageMessage::ListSessions { response_tx })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(OperationType::Slow, "ListSessions", response_rx.recv())
            .await
    }

    pub async fn save_checkpoint(
        &self,
        checkpoint: &crate::storage::rocksdb_storage::SessionCheckpoint,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::SaveCheckpoint {
                checkpoint: checkpoint.clone(),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(OperationType::Medium, "SaveCheckpoint", response_rx.recv())
            .await
    }

    pub async fn load_checkpoint(
        &self,
        checkpoint_id: uuid::Uuid,
    ) -> Result<crate::storage::rocksdb_storage::SessionCheckpoint, String> {
        let (response_tx, mut response_rx) =
            channel::<Result<crate::storage::rocksdb_storage::SessionCheckpoint, String>>(1);

        self.sender
            .send(StorageMessage::LoadCheckpoint {
                checkpoint_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            &format!("LoadCheckpoint {}", checkpoint_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::SaveWorkspaceMetadata {
                workspace_id,
                name: name.to_string(),
                description: description.to_string(),
                session_ids: session_ids.to_vec(),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "SaveWorkspaceMetadata",
            response_rx.recv(),
        )
        .await
    }

    pub async fn list_all_workspaces(
        &self,
    ) -> Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String> {
        let (response_tx, mut response_rx) =
            channel::<Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String>>(1);

        self.sender
            .send(StorageMessage::ListAllWorkspaces { response_tx })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(OperationType::Slow, "ListAllWorkspaces", response_rx.recv())
            .await
    }

    pub async fn delete_workspace(&self, workspace_id: Uuid) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::DeleteWorkspace {
                workspace_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("DeleteWorkspace {}", workspace_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::AddSessionToWorkspace {
                workspace_id,
                session_id,
                role,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            "AddSessionToWorkspace",
            response_rx.recv(),
        )
        .await
    }

    pub async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::RemoveSessionFromWorkspace {
                workspace_id,
                session_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            "RemoveSessionFromWorkspace",
            response_rx.recv(),
        )
        .await
    }

    pub async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::BatchSaveUpdates {
                session_id,
                updates,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("BatchSaveUpdates {}", session_id),
            response_rx.recv(),
        )
        .await
    }
}

impl StorageActor {
    pub async fn spawn(
        storage: Arc<dyn crate::storage::traits::Storage>,
        performance_monitor: Arc<PerformanceMonitor>,
    ) -> Result<StorageActorHandle, String> {
        let (sender, receiver) = unbounded_channel();

        let actor = Self {
            storage,
            receiver,
            performance_monitor,
            operation_count: AtomicU64::new(0),
            load_count: AtomicU64::new(0),
            save_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
        };

        // Create confirmation channel for startup synchronization
        let (startup_tx, startup_rx) = oneshot::channel();

        // Spawn async actor task with startup confirmation
        tokio::spawn(async move {
            // Send confirmation that actor is ready
            let _ = startup_tx.send(());
            actor.run_async().await;
        });

        // Wait for actor to be ready before returning handle
        startup_rx
            .await
            .map_err(|_| "Storage actor failed to start".to_string())?;

        Ok(StorageActorHandle { sender })
    }

    async fn run_async(mut self) {
        info!("Storage actor started (async)");

        while let Some(message) = self.receiver.recv().await {
            match message {
                StorageMessage::Shutdown => {
                    info!("Storage actor shutting down");
                    break;
                }
                msg => self.handle_message_async(msg).await,
            }
        }

        info!("Storage actor stopped");
    }

    async fn handle_message_async(&self, message: StorageMessage) {
        tracing::info!(
            "StorageActor: Handling message: {:?}",
            std::mem::discriminant(&message)
        );
        let _timer = self.performance_monitor.start_timer("storage_operation");
        self.operation_count.fetch_add(1, Ordering::Relaxed);

        match message {
            StorageMessage::LoadSession {
                session_id,
                response_tx,
            } => {
                self.load_count.fetch_add(1, Ordering::Relaxed);
                debug!("StorageActor: Loading session {}", session_id);
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => Ok(Some(session)),
                    Err(_) => Ok(None), // Session not found is OK, not an error
                };
                let _ = response_tx.send(result).await;
            }
            StorageMessage::SaveSession {
                session,
                response_tx,
            } => {
                self.save_count.fetch_add(1, Ordering::Relaxed);
                let result = self
                    .storage
                    .save_session(&session)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::DeleteSession {
                session_id,
                response_tx,
            } => {
                self.delete_count.fetch_add(1, Ordering::Relaxed);
                let result = self
                    .storage
                    .delete_session(session_id)
                    .await
                    .map(|_| true)
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::ListSessions { response_tx } => {
                self.load_count.fetch_add(1, Ordering::Relaxed);
                let result = self
                    .storage
                    .list_sessions()
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::GetStats { response_tx } => {
                let stats = StorageStats {
                    total_operations: self.operation_count.load(Ordering::Relaxed),
                    load_operations: self.load_count.load(Ordering::Relaxed),
                    save_operations: self.save_count.load(Ordering::Relaxed),
                    delete_operations: self.delete_count.load(Ordering::Relaxed),
                    avg_operation_time_ns: 0,
                    last_operation_timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("System time before UNIX epoch")
                        .as_secs(),
                };
                let _ = response_tx.send(stats).await;
            }
            StorageMessage::SaveCheckpoint {
                checkpoint,
                response_tx,
            } => {
                let result = self
                    .storage
                    .save_checkpoint(&checkpoint)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::LoadCheckpoint {
                checkpoint_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .load_checkpoint(checkpoint_id)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::SaveWorkspaceMetadata {
                workspace_id,
                name,
                description,
                session_ids,
                response_tx,
            } => {
                let result = self
                    .storage
                    .save_workspace_metadata(workspace_id, &name, &description, &session_ids)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::DeleteWorkspace {
                workspace_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .delete_workspace(workspace_id)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::AddSessionToWorkspace {
                workspace_id,
                session_id,
                role,
                response_tx,
            } => {
                let result = self
                    .storage
                    .add_session_to_workspace(workspace_id, session_id, role)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::RemoveSessionFromWorkspace {
                workspace_id,
                session_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .remove_session_from_workspace(workspace_id, session_id)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::ListAllWorkspaces { response_tx } => {
                let result = self
                    .storage
                    .list_workspaces()
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::BatchSaveUpdates {
                session_id,
                updates,
                response_tx,
            } => {
                let result = self
                    .storage
                    .batch_save_updates(session_id, updates)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::FindRelatedEntities {
                session_id,
                entity_name,
                response_tx,
            } => {
                // Load session and access its entity graph
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => {
                        let related = session.entity_graph.find_related_entities(&entity_name);
                        Ok(related)
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = response_tx.send(result).await;
            }
            StorageMessage::FindShortestPath {
                session_id,
                from_entity,
                to_entity,
                response_tx,
            } => {
                // Load session and access its entity graph
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => {
                        let path = session.entity_graph.find_shortest_path(&from_entity, &to_entity);
                        Ok(path)
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = response_tx.send(result).await;
            }
            StorageMessage::PersistSessionAndUpdate {
                session,
                session_id,
                updates,
            } => {
                self.save_count.fetch_add(1, Ordering::Relaxed);
                if let Err(e) = self
                    .storage
                    .save_session_with_updates(&session, session_id, updates)
                    .await
                {
                    warn!(
                        "Background persist failed for session {}: {}",
                        session_id, e
                    );
                } else {
                    debug!("Background persist completed for session {}", session_id);
                }
            }
            StorageMessage::Shutdown => {} // Handled in main loop
        }
    }
}

// Health and metrics structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub active_operations: usize,
    pub active_sessions: usize,
    pub circuit_breaker_open: bool,
    pub circuit_breaker_failures: u64,
    pub cache_hit_rate: f64,
    pub contexts_processed: u64,
    pub processing_errors: u64,
    pub storage_operations: u64,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManagerMetrics {
    pub session_count: usize,
    pub active_sessions: usize,
    pub total_operations: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_hit_rate: f64,
    pub cache_size: usize,
    pub cache_capacity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerStats {
    pub is_open: bool,
    pub failure_count: u64,
    pub success_count: u64,
    pub last_failure_timestamp: u64,
    pub last_success_timestamp: u64,
}

/// Extensions for ActiveSession to work with the memory system
impl ConversationMemorySystem {
    async fn add_context_update_to_session(
        session: &mut ActiveSession,
        description: String,
        metadata: Option<serde_json::Value>,
    ) -> Result<(String, crate::core::context_update::ContextUpdate), String> {
        use crate::core::context_update::{ContextUpdate, UpdateContent, UpdateType};

        tracing::info!(
            "add_context_update_to_session: Starting with description: '{}'",
            description
        );
        // Try to deserialize complete ContextUpdate from metadata
        let update = if let Some(metadata) = metadata {
            if let Ok(context_update) = serde_json::from_value::<ContextUpdate>(metadata) {
                tracing::info!(
                    "Using provided ContextUpdate with ID: {}",
                    context_update.id
                );
                context_update
            } else {
                tracing::info!("Failed to deserialize ContextUpdate, creating new one");
                let update_id = Uuid::new_v4();
                ContextUpdate {
                    id: update_id,
                    update_type: UpdateType::ConceptDefined,
                    content: UpdateContent {
                        title: "Incremental Update".to_string(),
                        description,
                        details: Vec::new(),
                        examples: Vec::new(),
                        implications: Vec::new(),
                    },
                    timestamp: chrono::Utc::now(),
                    related_code: None,
                    parent_update: None,
                    user_marked_important: false,
                    creates_entities: Vec::new(),
                    creates_relationships: Vec::new(),
                    references_entities: Vec::new(),
                }
            }
        } else {
            tracing::info!("No metadata provided, creating new ContextUpdate");
            let update_id = Uuid::new_v4();
            ContextUpdate {
                id: update_id,
                update_type: UpdateType::ConceptDefined,
                content: UpdateContent {
                    title: "Incremental Update".to_string(),
                    description,
                    details: Vec::new(),
                    examples: Vec::new(),
                    implications: Vec::new(),
                },
                timestamp: chrono::Utc::now(),
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: Vec::new(),
                creates_relationships: Vec::new(),
                references_entities: Vec::new(),
            }
        };

        tracing::info!("Using ContextUpdate with ID: {}", update.id);
        tracing::info!(
            "Update content - title: '{}', description: '{}', has_code: {}",
            update.content.title,
            update.content.description,
            update.related_code.is_some()
        );

        // Use the proper add_incremental_update method that handles entity extraction
        tracing::info!("Calling session.add_incremental_update...");
        let update_id = update.id;
        let update_clone = update.clone(); // Clone for returning to caller
        match session.add_incremental_update_fast(update).await {
            Ok(_) => {
                tracing::info!(
                    "add_context_update_to_session: Successfully added update {}",
                    update_id
                );
                Ok((update_id.to_string(), update_clone))
            }
            Err(e) => {
                tracing::error!(
                    "add_context_update_to_session: Failed to add incremental update: {e}"
                );
                Err(format!("Failed to add update: {e}"))
            }
        }
    }

    fn get_session_context_summary(session: &ActiveSession) -> String {
        let mut summary = Vec::new();

        // Recent updates from hot context
        for update in session.hot_context.iter().iter().rev().take(10) {
            summary.push(format!("- {}", update.content.description));
        }

        if summary.is_empty() {
            "No context available".to_string()
        } else {
            summary.join("\n")
        }
    }

    // Embeddings and Semantic Search Methods

    /// Lazy-initialize content vectorizer on first use with retry mechanism
    #[cfg(feature = "embeddings")]
    async fn ensure_vectorizer_initialized(&self) -> Result<Arc<ContentVectorizer>, String> {
        // Check if already initialized
        if let Some(vectorizer) = self.content_vectorizer.get() {
            return Ok(Arc::clone(vectorizer));
        }

        // Track initialization attempts for diagnostics
        let attempt = self
            .embedding_config_holder
            .init_attempt_count
            .fetch_add(1, Ordering::Relaxed)
            + 1;

        // Check if we've exceeded max retries
        if attempt > MAX_VECTORIZER_INIT_RETRIES as u64 + 1 {
            if let Some(last_error) = self.embedding_config_holder.last_init_error.read().as_ref() {
                return Err(format!(
                    "Vectorizer initialization failed after {} attempts. Last error: {}",
                    attempt - 1,
                    last_error
                ));
            }
            return Err(format!(
                "Vectorizer initialization failed after {} attempts",
                attempt - 1
            ));
        }

        info!(
            "Lazy-initializing content vectorizer (attempt {}/{})...",
            attempt,
            MAX_VECTORIZER_INIT_RETRIES + 1
        );

        // Try to initialize with retry logic
        let result: Result<&Arc<ContentVectorizer>, String> = self
            .content_vectorizer
            .get_or_try_init(|| async {
                let embedding_config = EmbeddingConfig {
                    model_type: self.embedding_config_holder.model_type,
                    max_batch_size: 32,
                    ..Default::default()
                };

                let vector_db_config = VectorDbConfig {
                    dimension: self.embedding_config_holder.vector_dimension,
                    max_vectors: self.embedding_config_holder.max_vectors_per_session,
                    ..Default::default()
                };

                let vectorizer_config = ContentVectorizerConfig {
                    embedding_config,
                    vector_db_config,
                    enable_cross_session_search: self
                        .embedding_config_holder
                        .cross_session_search_enabled,
                    ..Default::default()
                };

                let mut vectorizer = ContentVectorizer::new(vectorizer_config)
                    .await
                    .map_err(|e| format!("Failed to initialize content vectorizer: {}", e))?;

                // Set persistent storage for embedding persistence
                vectorizer.set_persistent_storage(self.vector_storage.clone());

                // Load persisted embeddings from storage immediately on initialization
                match vectorizer.load_all_embeddings_from_storage().await {
                    Ok(count) => {
                        if count > 0 {
                            info!("Loaded {} persisted embeddings from storage during initialization", count);
                        }
                    }
                    Err(e) => {
                        // We fail initialization if we can't load embeddings - this allows retry
                        return Err(format!("Failed to load persisted embeddings from storage: {}", e));
                    }
                }

                Ok(Arc::new(vectorizer))
            })
            .await;

        match result {
            Ok(vectorizer) => {
                info!(
                    "Content vectorizer initialized successfully on attempt {}",
                    attempt
                );
                // Clear any previous error
                *self.embedding_config_holder.last_init_error.write() = None;

                Ok(Arc::clone(vectorizer))
            }
            Err(e) => {
                // Store the error for diagnostics
                *self.embedding_config_holder.last_init_error.write() = Some(e.clone());
                error!(
                    "Vectorizer initialization failed on attempt {}: {}",
                    attempt, e
                );
                Err(e)
            }
        }
    }

    /// Lazy-initialize semantic query engine on first use
    #[cfg(feature = "embeddings")]
    pub async fn ensure_semantic_engine_initialized(
        &self,
    ) -> Result<Arc<crate::core::semantic_query_engine::SemanticQueryEngine>, String> {
        if let Some(engine) = self.semantic_query_engine.get() {
            return Ok(Arc::clone(engine));
        }

        // Ensure vectorizer is initialized first
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Initialize semantic engine
        self.semantic_query_engine
            .get_or_try_init(|| async {
                info!("Lazy-initializing semantic query engine...");

                use crate::core::semantic_query_engine::{
                    SemanticQueryConfig, SemanticQueryEngine,
                };

                let config = SemanticQueryConfig {
                    cross_session_enabled: self
                        .embedding_config_holder
                        .cross_session_search_enabled,
                    similarity_threshold: self.config.semantic_search_threshold,
                    ..Default::default()
                };

                let engine = SemanticQueryEngine::new((*vectorizer).clone(), config);

                Ok(Arc::new(engine))
            })
            .await
            .map(Arc::clone)
    }

    /// Vectorize a session's content (requires embeddings feature)
    #[cfg(feature = "embeddings")]
    pub async fn vectorize_session(&self, session_id: Uuid) -> Result<usize, String> {
        let _timer = self.performance_monitor.start_timer("vectorize_session");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Load session
        let session_result = self.get_session_internal(session_id).await?;
        let session = session_result.load();

        // Vectorize content
        match vectorizer.vectorize_session(&session).await {
            Ok(count) => {
                info!("Vectorized {} items for session {}", count, session_id);
                Ok(count)
            }
            Err(e) => {
                warn!("Failed to vectorize session {session_id}: {e}");
                Err(format!("Vectorization failed: {e}"))
            }
        }
    }

    /// Auto-vectorize only the latest update (incremental vectorization)
    /// This is much more efficient than re-vectorizing the entire session
    /// Includes retry mechanism for transient failures
    #[cfg(feature = "embeddings")]
    pub async fn auto_vectorize_if_enabled(&self, session_id: Uuid) -> Result<(), String> {
        if !self.config.enable_embeddings || !self.config.auto_vectorize_on_update {
            return Ok(());
        }

        // Lazy-initialize vectorizer if needed
        let vectorizer = match self.ensure_vectorizer_initialized().await {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to initialize vectorizer: {}", e);
                return Ok(()); // Don't fail the main operation
            }
        };

        // Load session
        let session_arc = match self.get_session_internal(session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "Failed to load session {} for vectorization: {}",
                    session_id, e
                );
                return Ok(()); // Don't fail the main operation
            }
        };

        let session = session_arc.load();

        // Retry loop for vectorization with exponential backoff
        let mut last_error = None;
        for attempt in 1..=MAX_VECTORIZATION_RETRIES {
            match vectorizer.vectorize_latest_update(&session).await {
                Ok(count) => {
                    info!(
                        "Incrementally vectorized {} update(s) for session {} (attempt {})",
                        count, session_id, attempt
                    );

                    // Invalidate only this session's cache entries instead of clearing all
                    // This is handled by the vectorizer internally now
                    if count > 0 {
                        if let Err(e) = vectorizer.invalidate_session_cache(session_id).await {
                            debug!(
                                "Cache invalidation for session {} (non-critical): {}",
                                session_id, e
                            );
                        }

                        // Persist session to save the updated vectorized_update_ids
                        if let Err(e) = self.storage_actor.save_session((**session).clone()).await {
                            warn!("Failed to persist session {} after vectorization: {}", session_id, e);
                        } else {
                            debug!("Session {} persisted after vectorization", session_id);
                        }
                    }

                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e.to_string());

                    if attempt < MAX_VECTORIZATION_RETRIES {
                        // Calculate exponential backoff delay
                        let delay_ms = VECTORIZATION_RETRY_DELAY_MS * (1 << (attempt - 1));
                        debug!(
                            "Vectorization attempt {} failed for session {}, retrying in {}ms: {}",
                            attempt, session_id, delay_ms, e
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        // All retries exhausted
        if let Some(error) = last_error {
            warn!(
                "Incremental vectorization failed for session {} after {} retries: {}",
                session_id, MAX_VECTORIZATION_RETRIES, error
            );
        }

        Ok(()) // Don't fail the main operation even after retries exhausted
    }

    /// Vectorize all sessions in the system with parallel processing
    /// Returns total number of vectorized items across all sessions and statistics
    /// Uses a semaphore to limit concurrent vectorization tasks
    #[cfg(feature = "embeddings")]
    pub async fn vectorize_all_sessions(&self) -> Result<(usize, usize, usize), String> {
        info!("Starting full vectorization of all sessions (parallel mode)");
        let start_time = std::time::Instant::now();

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Get all session IDs
        let session_ids = self.list_sessions().await?;
        let total_sessions = session_ids.len();

        if total_sessions == 0 {
            info!("No sessions found to vectorize");
            return Ok((0, 0, 0));
        }

        info!(
            "Found {} sessions to vectorize (max {} parallel tasks)",
            total_sessions, MAX_PARALLEL_VECTORIZATION
        );

        // Shared counters for parallel processing
        let total_vectorized = Arc::new(AtomicUsize::new(0));
        let successful_sessions = Arc::new(AtomicUsize::new(0));
        let failed_sessions = Arc::new(AtomicUsize::new(0));
        let processed_count = Arc::new(AtomicUsize::new(0));

        // Semaphore to limit concurrency
        let semaphore = Arc::new(Semaphore::new(MAX_PARALLEL_VECTORIZATION));

        // Process sessions in parallel with limited concurrency
        let mut handles = Vec::with_capacity(total_sessions);

        for session_id in session_ids {
            let vectorizer = Arc::clone(&vectorizer);
            let semaphore = Arc::clone(&semaphore);
            let total_vectorized = Arc::clone(&total_vectorized);
            let successful_sessions = Arc::clone(&successful_sessions);
            let failed_sessions = Arc::clone(&failed_sessions);
            let processed_count = Arc::clone(&processed_count);

            // Clone session_arc data we need before spawning
            let session_data = match self.get_session_internal(session_id).await {
                Ok(arc) => Some(arc.load().as_ref().clone()),
                Err(e) => {
                    failed_sessions.fetch_add(1, Ordering::Relaxed);
                    let count = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
                    warn!(
                        "[{}/{}] Failed to load session {}: {}",
                        count, total_sessions, session_id, e
                    );
                    None
                }
            };

            if let Some(session) = session_data {
                let handle = tokio::spawn(async move {
                    // Acquire semaphore permit
                    let _permit = semaphore.acquire().await.expect("Semaphore closed");

                    // Check if session already has embeddings
                    let already_vectorized = vectorizer.is_session_vectorized(session_id);
                    if already_vectorized {
                        let existing_count = vectorizer.count_session_embeddings(session_id);
                        debug!(
                            "Session {} already has {} embeddings, re-vectorizing...",
                            session_id, existing_count
                        );
                    }

                    // Vectorize the session
                    match vectorizer.vectorize_session(&session).await {
                        Ok(count) => {
                            total_vectorized.fetch_add(count, Ordering::Relaxed);
                            successful_sessions.fetch_add(1, Ordering::Relaxed);
                            let processed = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
                            info!(
                                "[{}/{}] Vectorized {} items for session {}",
                                processed, total_sessions, count, session_id
                            );
                        }
                        Err(e) => {
                            failed_sessions.fetch_add(1, Ordering::Relaxed);
                            let processed = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
                            warn!(
                                "[{}/{}] Failed to vectorize session {}: {}",
                                processed, total_sessions, session_id, e
                            );
                        }
                    }
                });

                handles.push(handle);
            }
        }

        // Wait for all tasks to complete
        for handle in handles {
            let _ = handle.await;
        }

        // Clear query cache after bulk vectorization
        if let Err(e) = vectorizer.clear_query_cache().await {
            warn!(
                "Failed to clear query cache after bulk vectorization: {}",
                e
            );
        }

        let elapsed = start_time.elapsed();
        let total = total_vectorized.load(Ordering::Relaxed);
        let success = successful_sessions.load(Ordering::Relaxed);
        let failed = failed_sessions.load(Ordering::Relaxed);

        info!(
            "Bulk vectorization complete in {:.2}s: {} total items across {} successful sessions ({} failed)",
            elapsed.as_secs_f64(),
            total,
            success,
            failed
        );

        Ok((total, success, failed))
    }

    /// Perform semantic search across all sessions
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_global(
        &self,
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("semantic_search_global");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(limit.unwrap_or(20)),
            date_range,
            recency_bias,
        };

        match vectorizer
            .semantic_search(query, limit.unwrap_or(20), None, options)
            .await
        {
            Ok(results) => Ok(results),
            Err(e) => Err(format!("Semantic search failed: {e}")),
        }
    }

    /// Perform semantic search within a specific session
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_session(
        &self,
        session_id: Uuid,
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("semantic_search_session");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Auto-load session if not already loaded (get_session_internal handles this)
        // This ensures the session is in memory before vectorization
        // We keep a reference to use for Graph-RAG enrichment
        let session_arc = self.get_session_internal(session_id).await?;

        // Auto-vectorize if session hasn't been vectorized yet
        if !vectorizer.is_session_vectorized(session_id) {
            info!(
                "Session {} not vectorized, auto-vectorizing before search",
                session_id
            );
            if let Err(e) = self.vectorize_session(session_id).await {
                warn!(
                    "Auto-vectorization failed for session {}: {}",
                    session_id, e
                );
                // Continue anyway - search might still work with partial data
            }
        }

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(limit.unwrap_or(20)),
            date_range,
            recency_bias,
        };

        match vectorizer
            .semantic_search(query, limit.unwrap_or(20), Some(session_id), options)
            .await
        {
            Ok(results) => {
                // Helper to extract potential entity names from text
                // Extract all words > 3 chars (will be validated against graph)
                let extract_entities = |text: &str| -> Vec<String> {
                    text.split_whitespace()
                        .filter_map(|w| {
                            let clean = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
                            if clean.len() > 3 {
                                Some(clean.to_lowercase())
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                // ADVANCED GRAPH-RAG LOGIC
                // Use the already-loaded session's entity graph directly (no async message passing)
                let session = session_arc.load();
                let entity_graph = &session.entity_graph;
                let graph_size = entity_graph.entity_count();

                tracing::debug!("Graph-RAG: Starting enrichment for {} results (graph has {} entities)",
                    results.len(), graph_size);

                // 1. Identify core concepts in the QUERY
                let query_entities = extract_entities(query);
                tracing::debug!("Graph-RAG: Extracted {} query entities: {:?}", query_entities.len(), query_entities);
                let mut global_graph_map = std::collections::HashMap::new();

                // 2. Expand Query Context via Graph Traversal (direct graph access - O(degree) per entity)
                for q_entity in &query_entities {
                    let relations = entity_graph.find_related_entities(q_entity);
                    if !relations.is_empty() {
                        tracing::debug!("Graph-RAG: Entity '{}' -> {} relations: {:?}",
                            q_entity, relations.len(), relations.iter().take(3).collect::<Vec<_>>());
                        global_graph_map.insert(q_entity.clone(), relations);
                    } else {
                        tracing::debug!("Graph-RAG: Entity '{}' not found in graph", q_entity);
                    }
                }

                // 3. Synthesize Global Insights
                let mut graph_insights = String::new();
                if !global_graph_map.is_empty() {
                    graph_insights.push_str("\n[System Knowledge Map]:\n");
                    for (entity, rels) in &global_graph_map {
                        graph_insights.push_str(&format!("- {} is central to: {}\n", entity, rels.join(", ")));
                    }
                }

                // 4. Enrich results and analyze cross-references (direct graph access)
                let mut enriched_results = Vec::new();
                for mut result in results.into_iter() {
                    // Local enrichment (neighbors of entities in this specific chunk)
                    let chunk_entities = extract_entities(&result.text_content);
                    // De-duplicate
                    let mut unique_chunk_entities = chunk_entities.clone();
                    unique_chunk_entities.sort();
                    unique_chunk_entities.dedup();

                    let mut local_rels = Vec::new();
                    for entity in unique_chunk_entities.iter().take(2) {
                        // Skip if already in global map
                        if global_graph_map.contains_key(entity) { continue; }

                        // Direct graph access instead of async message
                        let relations = entity_graph.find_related_entities(entity);
                        if !relations.is_empty() {
                            // Limit relations
                            let limited_rels: Vec<_> = relations.iter().take(5).cloned().collect();
                            local_rels.push(format!("{}: {}", entity, limited_rels.join(", ")));
                        }
                    }

                    if !local_rels.is_empty() {
                        result.text_content = format!("{}\n(Graph expansion: {})", result.text_content, local_rels.join(" | "));
                    }

                    enriched_results.push(result);
                }

                // 5. Deep Graph Analysis (Pathfinding between top results) - direct graph access
                // If we have at least 2 results, check if they are connected
                if enriched_results.len() >= 2 {
                    let top1_entities = extract_entities(&enriched_results[0].text_content);
                    let top2_entities = extract_entities(&enriched_results[1].text_content);

                    if let (Some(e1), Some(e2)) = (top1_entities.first(), top2_entities.first()) {
                        if e1 != e2 {
                            // Direct pathfinding call
                            if let Some(path) = entity_graph.find_shortest_path(e1, e2) {
                                if path.len() > 2 { // Valid path found (more than just start/end)
                                    tracing::debug!("Graph-RAG: Found path {} -> {}: {:?}", e1, e2, path);
                                    graph_insights.push_str(&format!("\n[Structural Insight]: Found connection: {}\n", path.join(" -> ")));
                                }
                            }
                        }
                    }
                }

                // Prepend insights to the first result
                if !graph_insights.is_empty() && !enriched_results.is_empty() {
                    tracing::debug!("Graph-RAG: Adding insights to first result: {} chars", graph_insights.len());
                    enriched_results[0].text_content = format!("{}{}\n---\n", graph_insights, enriched_results[0].text_content);
                } else {
                    tracing::debug!("Graph-RAG: No insights generated (global_map: {} entries)", global_graph_map.len());
                }

                Ok(enriched_results)
            },
            Err(e) => Err(format!("Session semantic search failed: {e}")),
        }
    }

    /// Find related content across sessions
    #[cfg(feature = "embeddings")]
    pub async fn find_related_content(
        &self,
        session_id: Uuid,
        topic: &str,
        limit: Option<usize>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self.performance_monitor.start_timer("find_related_content");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Auto-load session if not already loaded
        let session_result = self.get_session_internal(session_id).await?;
        let session = session_result.load();

        // Auto-vectorize if session hasn't been vectorized yet
        if !vectorizer.is_session_vectorized(session_id) {
            info!(
                "Session {} not vectorized, auto-vectorizing before related content search",
                session_id
            );
            if let Err(e) = self.vectorize_session(session_id).await {
                warn!(
                    "Auto-vectorization failed for session {}: {}",
                    session_id, e
                );
                // Continue anyway - search might still work with partial data
            }
        }

        match vectorizer
            .find_related_content(&session, topic, limit.unwrap_or(10))
            .await
        {
            Ok(results) => Ok(results),
            Err(e) => Err(format!("Related content search failed: {e}")),
        }
    }

    /// Perform semantic search across multiple sessions
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_multisession(
        &self,
        session_ids: &[Uuid],
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("semantic_search_multisession");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(limit.unwrap_or(20)),
            date_range,
            recency_bias,
        };

        match vectorizer
            .semantic_search_multisession(query, limit.unwrap_or(20), session_ids, options)
            .await
        {
            Ok(results) => Ok(results),
            Err(e) => Err(format!("Multisession semantic search failed: {e}")),
        }
    }

    /// Get vectorization statistics
    #[cfg(feature = "embeddings")]
    pub fn get_vectorization_stats(
        &self,
    ) -> Result<std::collections::HashMap<String, usize>, String> {
        // Check if vectorizer has been initialized
        if let Some(vectorizer) = self.content_vectorizer.get() {
            Ok(vectorizer.get_vectorization_stats())
        } else {
            Err("Embeddings not initialized yet (call any vectorization method first)".to_string())
        }
    }

    /// Check if embeddings are enabled and initialized
    pub fn embeddings_enabled(&self) -> bool {
        self.config.enable_embeddings && cfg!(feature = "embeddings") && {
            #[cfg(feature = "embeddings")]
            {
                self.content_vectorizer.get().is_some()
            }
            #[cfg(not(feature = "embeddings"))]
            {
                false
            }
        }
    }

    /// Enable embeddings at runtime (requires restart to initialize components)
    pub async fn enable_embeddings_config(&mut self) -> Result<(), String> {
        if !cfg!(feature = "embeddings") {
            return Err("Embeddings feature not compiled in".to_string());
        }

        self.config.enable_embeddings = true;
        Ok(())
    }

    /// Configure embedding model type
    pub async fn set_embedding_model(&mut self, model_type: String) -> Result<(), String> {
        self.config.embeddings_model_type = model_type;
        Ok(())
    }

    /// Clear query cache to prevent stale vector IDs after restart
    ///
    /// This should be called on daemon startup to ensure cached query results
    /// don't reference vector IDs from before the restart, which would cause
    /// incorrect similarity calculations.
    pub async fn clear_query_cache(&self) -> Result<(), String> {
        #[cfg(feature = "embeddings")]
        {
            if let Some(vectorizer) = self.content_vectorizer.get() {
                vectorizer
                    .clear_query_cache()
                    .await
                    .map_err(|e| format!("Failed to clear query cache: {}", e))?;
                info!("Query cache cleared successfully");
            }
        }
        Ok(())
    }
}

/// Wrapper to make ArcSwap compatible with RwLock API for MCP compatibility
pub struct RwLockCompatWrapper<T> {
    inner: Arc<ArcSwap<T>>,
}

impl<T> RwLockCompatWrapper<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(value))),
        }
    }

    pub fn from_arc_swap(arc_swap: Arc<ArcSwap<T>>) -> Self {
        Self { inner: arc_swap }
    }

    /// Read access - returns a guard that derefs to T
    pub async fn read(&self) -> RwLockReadGuard<T> {
        RwLockReadGuard {
            inner: Arc::clone(&*self.inner.load()),
        }
    }

    /// Write access - returns a guard for atomic updates
    pub async fn write(&self) -> RwLockWriteGuard<T>
    where
        T: Clone,
    {
        RwLockWriteGuard {
            arc_swap: Arc::clone(&self.inner),
            current: Arc::clone(&*self.inner.load()),
        }
    }
}

/// Read guard that provides deref access to the value
pub struct RwLockReadGuard<T> {
    inner: Arc<T>,
}

impl<T> std::ops::Deref for RwLockReadGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Write guard that enables atomic updates
pub struct RwLockWriteGuard<T>
where
    T: Clone,
{
    arc_swap: Arc<ArcSwap<T>>,
    current: Arc<T>,
}

impl<T> std::ops::Deref for RwLockWriteGuard<T>
where
    T: Clone,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.current
    }
}

// Note: DerefMut is intentionally NOT implemented for RwLockWriteGuard
// to prevent unsafe mutable access patterns. Use the update() method instead.

impl<T> RwLockWriteGuard<T>
where
    T: Clone,
{
    /// Update the value atomically
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        let mut new_value = (*self.current).clone();
        f(&mut new_value);
        self.arc_swap.store(Arc::new(new_value));
    }

    /// Replace the entire value atomically
    pub fn replace(&self, new_value: T) {
        self.arc_swap.store(Arc::new(new_value));
    }
}

/// Update get_session to return RwLock-compatible wrapper
/// Comprehensive API compatibility layer for MCP tools
impl ConversationMemorySystem {
    /// Storage compatibility - returns a wrapper that behaves like Arc<RwLock<Storage>>
    pub fn storage(&self) -> StorageCompatibilityWrapper<'_> {
        StorageCompatibilityWrapper {
            actor: &self.storage_actor,
        }
    }

    /// Get session with RwLock-like API for backwards compatibility
    pub async fn get_session_compat(
        &self,
        session_id: Uuid,
    ) -> Result<SessionCompatibilityWrapper, anyhow::Error> {
        let session_arc = self
            .get_session_internal(session_id)
            .await
            .map_err(anyhow::Error::msg)?;

        Ok(SessionCompatibilityWrapper::new(session_arc))
    }

    async fn get_session_internal(
        &self,
        session_id: Uuid,
    ) -> Result<Arc<ArcSwap<ActiveSession>>, String> {
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

/// Storage compatibility wrapper
pub struct StorageCompatibilityWrapper<'a> {
    actor: &'a StorageActorHandle,
}

impl<'a> StorageCompatibilityWrapper<'a> {
    pub async fn read(&self) -> &StorageActorHandle {
        self.actor
    }

    pub async fn write(&self) -> &StorageActorHandle {
        self.actor
    }

    // Workspace storage proxy methods
    pub async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<(), String> {
        self.actor
            .save_workspace_metadata(workspace_id, name, description, session_ids)
            .await
    }

    pub async fn delete_workspace(&self, workspace_id: Uuid) -> Result<(), String> {
        self.actor.delete_workspace(workspace_id).await
    }

    pub async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
    ) -> Result<(), String> {
        self.actor
            .add_session_to_workspace(workspace_id, session_id, role)
            .await
    }

    pub async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), String> {
        self.actor
            .remove_session_from_workspace(workspace_id, session_id)
            .await
    }
}

/// Session compatibility wrapper that mimics Arc<RwLock<ActiveSession>>
pub struct SessionCompatibilityWrapper {
    inner: Arc<ArcSwap<ActiveSession>>,
}

impl SessionCompatibilityWrapper {
    pub fn new(inner: Arc<ArcSwap<ActiveSession>>) -> Self {
        Self { inner }
    }

    pub async fn read(&self) -> SessionReadGuard {
        SessionReadGuard {
            inner: Arc::clone(&*self.inner.load()),
        }
    }

    pub async fn write(&self) -> SessionWriteGuard {
        SessionWriteGuard {
            arc_swap: Arc::clone(&self.inner),
            current: Arc::clone(&*self.inner.load()),
        }
    }
}

/// Session read guard
pub struct SessionReadGuard {
    inner: Arc<ActiveSession>,
}

impl std::ops::Deref for SessionReadGuard {
    type Target = ActiveSession;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Session write guard that enables atomic updates
pub struct SessionWriteGuard {
    arc_swap: Arc<ArcSwap<ActiveSession>>,
    current: Arc<ActiveSession>,
}

impl std::ops::Deref for SessionWriteGuard {
    type Target = ActiveSession;

    fn deref(&self) -> &Self::Target {
        &self.current
    }
}

impl SessionWriteGuard {
    /// Update the session atomically - required for modifications
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut ActiveSession),
    {
        let mut new_session = (*self.current).clone();
        f(&mut new_session);
        self.arc_swap.store(Arc::new(new_session));
    }
}

/// Convenience macro for timing memory operations
#[macro_export]
macro_rules! time_memory_operation {
    ($system:expr, $operation:expr, $code:block) => {{
        let _timer = $system.performance_monitor.start_timer($operation);
        $code
    }};
}
