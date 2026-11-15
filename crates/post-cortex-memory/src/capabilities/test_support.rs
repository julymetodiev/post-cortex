// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Memory instances for the capability tests.

use std::sync::Arc;

use crate::capabilities::Memory;
use crate::memory_system::{MemoryRuntime, SystemConfig};
use crate::pipeline::PipelineConfig;

/// The services over an existing runtime (e.g. one a test backend
/// started).
pub(crate) fn memory(rt: &Arc<MemoryRuntime>) -> Memory {
    memory_with(rt, PipelineConfig::default())
}

/// [`memory`] with an explicit pipeline configuration.
pub(crate) fn memory_with(rt: &Arc<MemoryRuntime>, pipeline: PipelineConfig) -> Memory {
    let ports = crate::capabilities::MemoryPorts {
        stores: rt.stores.clone(),
        index: rt.index().cloned(),
        engine: rt.engine().cloned(),
        reranker: rt.reranker().cloned(),
        accurate_reranker: rt.accurate_reranker().cloned(),
        archive: Arc::new(post_cortex_storage::JsonArchive),
        blobs: Some(Arc::new(post_cortex_blobs::ObjectBlobs::in_memory())),
    };
    Memory::from_runtime(
        Arc::clone(rt),
        &ports,
        pipeline,
        post_cortex_core::redact::Redactor::new(&rt.config.redaction).unwrap(),
    )
}

/// A RocksDB-backed instance without embeddings in a temp dir.
pub(crate) async fn rocksdb_memory() -> (Memory, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_system::backends_tests::rocksdb_memory_in(
        &dir,
        SystemConfig {
            enable_embeddings: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (memory, dir)
}
