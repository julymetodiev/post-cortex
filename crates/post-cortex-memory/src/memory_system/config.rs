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
    /// Formerly the maximum number of vectors per session. Never enforced;
    /// the in-memory vector DB is uncapped. Kept for compatibility.
    #[deprecated(note = "not enforced: the vector DB is uncapped; the value is ignored")]
    pub max_vectors_per_session: usize,
    /// Directory where vector index data is stored
    pub data_directory: String,
    /// Whether cross-session semantic search is enabled
    pub cross_session_search_enabled: bool,
    /// Cross-encoder to rerank search results with, if any
    pub reranker_model: Option<String>,
    /// Voyage API key and model; Voyage reranks when the key is set
    pub voyage: Option<(String, String)>,
    /// Default search mode (`fast`, `local`, `accurate`), if configured
    pub search_mode: Option<String>,
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
    /// Formerly the maximum number of vectors per session. It was never
    /// enforced and is ignored: the in-memory vector DB is uncapped
    /// (`VectorDbConfig::max_vectors = usize::MAX`), and a per-session cap
    /// would silently drop searchable content. Kept so existing struct
    /// literals keep compiling; it will be removed in a future release.
    #[deprecated(note = "not enforced: the vector DB is uncapped; the value is ignored")]
    pub max_vectors_per_session: usize,
    /// Minimum similarity score for semantic search results
    pub semantic_search_threshold: f32,
    /// Whether to automatically vectorize content on updates
    pub auto_vectorize_on_update: bool,
    /// Whether cross-session semantic search is enabled
    pub cross_session_search_enabled: bool,
    /// HuggingFace id of a cross-encoder that reranks semantic search
    /// results (e.g. `cross-encoder/ms-marco-MiniLM-L4-v2`). `None` keeps the
    /// fused vector + BM25 order. Needs the `rerank` feature.
    pub reranker_model: Option<String>,
    /// Voyage AI API key. When set, Voyage reranks search results (with the
    /// local `reranker_model` as fallback); candidate text is sent to Voyage.
    /// Needs the `rerank-voyage` feature.
    pub voyage_api_key: Option<String>,
    /// Voyage rerank model (default `rerank-3-lite`).
    pub voyage_model: String,
    /// Search mode when a search does not name one: `fast`, `local` or
    /// `accurate`. Default: the best the configured rerankers allow.
    pub search_mode: Option<String>,
    /// Updates returned on each side of every update hit, from the same
    /// session (0 = hits only). Default 1.
    pub neighbour_radius: usize,
    /// Secret / personal-data redaction on the canonical write path
    pub redaction: post_cortex_core::redact::RedactionConfig,
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

impl SystemConfig {
    /// Whether the storage backend has a full-text (BM25) index of its own.
    /// SurrealDB does; with RocksDB the vector DB keeps one in memory.
    pub fn storage_has_full_text(&self) -> bool {
        #[cfg(feature = "surrealdb-storage")]
        {
            self.storage_backend == post_cortex_storage::traits::StorageBackendType::SurrealDB
        }
        #[cfg(not(feature = "surrealdb-storage"))]
        {
            false
        }
    }
}

impl Default for SystemConfig {
    // Still initialises the deprecated `max_vectors_per_session`.
    #[allow(deprecated)]
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
            // Embeddings defaults — matches EmbeddingModelType::default()
            // (PotionMultilingual / model2vec). Breaking change in 0.3.0;
            // pin "MultilingualMiniLM" + 384 if you need the BERT path.
            enable_embeddings: true,
            embeddings_model_type: "PotionMultilingual".to_string(),
            vector_dimension: 256, // potion-multilingual-128M outputs 256-dim vectors
            max_vectors_per_session: 1000,
            semantic_search_threshold: 0.7,
            auto_vectorize_on_update: true,
            cross_session_search_enabled: true,
            reranker_model: None,
            voyage_api_key: None,
            voyage_model: "rerank-3-lite".to_string(),
            search_mode: None,
            neighbour_radius: 1,
            redaction: post_cortex_core::redact::RedactionConfig::default(),
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
