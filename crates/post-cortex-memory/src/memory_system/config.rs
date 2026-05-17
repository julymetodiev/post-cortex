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
    /// Embedding model type to use
    pub model_type: EmbeddingModelType,
    /// Dimension of the embedding vectors
    pub vector_dimension: usize,
    /// Maximum number of vectors stored per session
    pub max_vectors_per_session: usize,
    /// Directory where vector index data is stored
    pub data_directory: String,
    /// Whether cross-session semantic search is enabled
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
    /// Maximum number of entries in the hot context tier
    pub max_hot_context_size: usize,
    /// Maximum number of entries in the warm context tier
    pub max_warm_context_size: usize,
    /// Threshold at which context is compressed
    pub context_compression_threshold: usize,
    /// Session inactivity timeout in minutes
    pub session_timeout_minutes: u64,
    /// Storage operation timeout in seconds
    pub storage_timeout_seconds: u64,
    /// Maximum number of entries in the session cache
    pub cache_capacity: usize,
    /// Whether to enable performance monitoring
    pub enable_performance_monitoring: bool,
    /// Number of failures before the circuit breaker opens
    pub circuit_breaker_failure_threshold: u64,
    /// Seconds the circuit breaker stays open before allowing retries
    pub circuit_breaker_timeout_seconds: u64,
    /// Filesystem path where persistent data is stored
    pub data_directory: String,
    /// Whether embedding/vectorization features are enabled
    pub enable_embeddings: bool,
    /// Name of the embedding model to use
    pub embeddings_model_type: String,
    /// Dimensionality of embedding vectors
    pub vector_dimension: usize,
    /// Maximum number of vectors per session
    pub max_vectors_per_session: usize,
    /// Minimum similarity score for semantic search results
    pub semantic_search_threshold: f32,
    /// Whether to automatically vectorize content on updates
    pub auto_vectorize_on_update: bool,
    /// Whether cross-session semantic search is enabled
    pub cross_session_search_enabled: bool,
    /// Storage backend type
    #[cfg(feature = "surrealdb-storage")]
    pub storage_backend: post_cortex_storage::traits::StorageBackendType,
    /// SurrealDB connection endpoint
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_endpoint: Option<String>,
    /// SurrealDB authentication username
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_username: Option<String>,
    /// SurrealDB authentication password
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_password: Option<String>,
    /// SurrealDB namespace
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb_namespace: Option<String>,
    /// SurrealDB database name
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

