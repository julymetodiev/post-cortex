// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A storage backend behind a network: every session write and vector call
//! waits a fixed round trip first, and is counted. Stands for a SurrealDB
//! server, where the round trip (not the in-process work) bounds a worker
//! that stores one item per call.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::VectorMetadata;
use post_cortex_ports::VectorStore;
use uuid::Uuid;

use super::backends_tests::Backend;
use super::persister::{PersistSink, StorageSink};
use super::{MemoryRuntime, SystemConfig};

/// Calls made through a [`Remote`].
#[derive(Default)]
pub(crate) struct Calls {
    /// Session writes (one per persist the persister ran).
    pub(crate) session_writes: AtomicUsize,
    /// Vector store writes (single or batch).
    pub(crate) vector_writes: AtomicUsize,
    /// Vector store deletes (each a batch).
    pub(crate) vector_deletes: AtomicUsize,
}

/// `inner` with `rtt` before each call; see the module docs.
pub(crate) struct Remote<T> {
    inner: T,
    rtt: Duration,
    calls: Arc<Calls>,
}

impl<T> Remote<T> {
    async fn round_trip(&self) {
        if !self.rtt.is_zero() {
            tokio::time::sleep(self.rtt).await;
        }
    }
}

/// A memory system with `config` on `backend` behind a [`Remote`] of
/// `rtt`, and its call counts.
pub(crate) async fn remote_system(
    backend: &Backend,
    config: SystemConfig,
    engine: Arc<dyn post_cortex_ports::EmbeddingEngine>,
    rtt: Duration,
) -> (MemoryRuntime, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let stores = backend.stores();
    let sink = Remote {
        inner: StorageSink::new(Arc::clone(&stores.sessions)),
        rtt,
        calls: Arc::clone(&calls),
    };
    let vectors = Remote {
        inner: Arc::clone(backend.vectors()),
        rtt,
        calls: Arc::clone(&calls),
    };
    let system = MemoryRuntime::new_with_sink(
        backend.ports(Some(engine), Some(Arc::new(vectors))),
        Arc::new(sink),
        config,
    )
    .await
    .unwrap();
    (system, calls)
}

#[async_trait]
impl PersistSink for Remote<StorageSink> {
    async fn write(
        &self,
        session: &ActiveSession,
        changes: post_cortex_ports::SessionChanges,
    ) -> Result<(), post_cortex_ports::PortError> {
        self.round_trip().await;
        self.calls.session_writes.fetch_add(1, Ordering::SeqCst);
        self.inner.write(session, changes).await
    }

    async fn delete(&self, session_id: Uuid) -> Result<bool, post_cortex_ports::PortError> {
        self.round_trip().await;
        self.inner.delete(session_id).await
    }
}

#[async_trait]
impl VectorStore for Remote<Arc<dyn VectorStore>> {
    async fn upsert_vectors(
        &self,
        vectors: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> post_cortex_ports::error::Result<()> {
        self.round_trip().await;
        self.calls.vector_writes.fetch_add(1, Ordering::SeqCst);
        self.inner.upsert_vectors(vectors).await
    }

    async fn delete_vectors(
        &self,
        session_id: &str,
        content_ids: &[String],
    ) -> post_cortex_ports::error::Result<usize> {
        self.round_trip().await;
        self.calls.vector_deletes.fetch_add(1, Ordering::SeqCst);
        self.inner.delete_vectors(session_id, content_ids).await
    }

    async fn has_session_embeddings(
        &self,
        session_id: &str,
    ) -> post_cortex_ports::error::Result<bool> {
        self.round_trip().await;
        self.inner.has_session_embeddings(session_id).await
    }

    async fn count_session_embeddings(
        &self,
        session_id: &str,
    ) -> post_cortex_ports::error::Result<usize> {
        self.round_trip().await;
        self.inner.count_session_embeddings(session_id).await
    }

    async fn total_count(&self) -> post_cortex_ports::error::Result<usize> {
        self.round_trip().await;
        self.inner.total_count().await
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> post_cortex_ports::error::Result<Vec<(Vec<f32>, VectorMetadata)>> {
        self.round_trip().await;
        self.inner.get_session_vectors(session_id).await
    }

    async fn get_all_vectors(
        &self,
    ) -> post_cortex_ports::error::Result<Vec<(Vec<f32>, VectorMetadata)>> {
        self.round_trip().await;
        self.inner.get_all_vectors().await
    }
}
