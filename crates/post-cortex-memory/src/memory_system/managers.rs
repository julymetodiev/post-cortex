use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize};

/// Incremental context processor (metric container)
pub struct IncrementalContextProcessor {
    /// Number of context updates processed successfully
    pub contexts_processed: Arc<AtomicU64>,
    /// Number of processing errors encountered
    pub processing_errors: Arc<AtomicU64>,
    /// Cumulative processing time in nanoseconds
    pub total_processing_time_ns: Arc<AtomicU64>,
    /// Cached average processing time in nanoseconds
    pub avg_processing_time_ns: Arc<AtomicU64>,
}

/// Graph manager (metric container)
pub struct SimpleGraphManager {
    /// Number of entities in the knowledge graph
    pub entities_count: Arc<AtomicUsize>,
    /// Number of relationships in the knowledge graph
    pub relationships_count: Arc<AtomicUsize>,
    /// Total number of graph operations performed
    pub graph_operations: Arc<AtomicU64>,
    /// Number of graph update operations performed
    pub graph_updates: Arc<AtomicU64>,
}
