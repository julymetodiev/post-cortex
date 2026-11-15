use std::sync::atomic::AtomicU64;

/// State of the lazy vectorizer start
pub struct EmbeddingConfigHolder {
    /// Whether cross-session semantic search is enabled
    pub cross_session_search_enabled: bool,
    /// Default search mode (`fast`, `local`, `accurate`), if configured
    pub search_mode: Option<String>,
    /// Tracks initialization attempts for retry mechanism
    pub init_attempt_count: AtomicU64,
    /// Last initialization error (for diagnostics)
    pub last_init_error: parking_lot::RwLock<Option<String>>,
}

/// The default of [`SystemConfig::max_import_bytes`]: 1 GiB.
pub const DEFAULT_MAX_IMPORT_BYTES: u64 = 1 << 30;

/// Capability configuration of a memory instance. The adapters (storage
/// backend, data directory, embedding model and its dimension, rerankers)
/// are the composition root's configuration (see [`crate::MemoryPorts`]).
#[derive(Debug, Clone)]
pub struct SystemConfig {
    /// Maximum number of entries in the session cache
    pub cache_capacity: usize,
    /// Whether embedding/vectorization features are enabled (needs an
    /// embedding engine and a search index among the ports)
    pub enable_embeddings: bool,
    /// Whether to automatically vectorize content on updates
    pub auto_vectorize_on_update: bool,
    /// Whether cross-session semantic search is enabled
    pub cross_session_search_enabled: bool,
    /// Search mode when a search does not name one: `fast`, `local` or
    /// `accurate`. Default: the best the configured rerankers allow.
    pub search_mode: Option<String>,
    /// Updates returned on each side of every update hit, from the same
    /// session (0 = hits only). Default 1.
    pub neighbour_radius: usize,
    /// Secret / personal-data redaction on the canonical write path
    pub redaction: post_cortex_core::redact::RedactionConfig,
    /// Largest archive an import takes, in bytes (it is spooled to a
    /// temporary file, never held in memory). Default 1 GiB.
    pub max_import_bytes: u64,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 1000,
            enable_embeddings: true,
            auto_vectorize_on_update: true,
            cross_session_search_enabled: true,
            search_mode: None,
            neighbour_radius: 1,
            redaction: post_cortex_core::redact::RedactionConfig::default(),
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
        }
    }
}
