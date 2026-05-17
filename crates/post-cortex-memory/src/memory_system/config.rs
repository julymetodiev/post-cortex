use std::sync::atomic::AtomicU64;
use std::time::Duration;

#[cfg(feature = "embeddings")]
use post_cortex_embeddings::EmbeddingModelType;

// Timeout constants used by OperationType
const TIMEOUT_FAST_MS: u64 = 5_000;
const TIMEOUT_MEDIUM_MS: u64 = 30_000;
const TIMEOUT_SLOW_MS: u64 = 120_000;
const TIMEOUT_VECTORIZATION_MS: u64 = 300_000;

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
    pub storage_backend: post_cortex_storage::traits::StorageBackendType,
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
            storage_backend: post_cortex_storage::traits::StorageBackendType::RocksDB,
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

