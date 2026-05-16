use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize};

/// Incremental context processor (metric container)
pub struct IncrementalContextProcessor {
    pub contexts_processed: Arc<AtomicU64>,
    pub processing_errors: Arc<AtomicU64>,
    pub total_processing_time_ns: Arc<AtomicU64>,
    pub avg_processing_time_ns: Arc<AtomicU64>,
}

/// Graph manager (metric container)
pub struct SimpleGraphManager {
    pub entities_count: Arc<AtomicUsize>,
    pub relationships_count: Arc<AtomicUsize>,
    pub graph_operations: Arc<AtomicU64>,
    pub graph_updates: Arc<AtomicU64>,
}
