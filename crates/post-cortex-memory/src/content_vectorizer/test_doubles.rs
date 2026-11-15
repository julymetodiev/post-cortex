// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Test doubles of the vectorizer's ports: an in-memory vector store with
//! an optional full-text index, and a reranker.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use post_cortex_ports::PortError;
use uuid::Uuid;

use post_cortex_ports::{Reranker, VectorMetadata, VectorStore};

/// Stored embeddings by (session, content id).
type Rows = HashMap<(String, String), (Vec<f32>, VectorMetadata)>;

/// In-memory persisted embeddings, keyed like RocksDB: (session, content id).
#[derive(Default)]
pub(crate) struct FakeStorage {
    pub(crate) rows: Mutex<Rows>,
    pub(crate) fail_writes: AtomicBool,
    pub(crate) batch_calls: AtomicUsize,
}

impl FakeStorage {
    pub(crate) fn has(&self, session: Uuid, content_id: &str) -> bool {
        self.rows
            .lock()
            .contains_key(&(session.to_string(), content_id.to_string()))
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.lock().len()
    }

    pub(crate) fn put(
        &self,
        vector: Vec<f32>,
        metadata: VectorMetadata,
    ) -> Result<String, PortError> {
        if self.fail_writes.load(Ordering::Relaxed) {
            return Err(PortError::unavailable("storage down"));
        }
        let id = metadata.id.clone();
        self.rows
            .lock()
            .insert((metadata.source.clone(), id.clone()), (vector, metadata));
        Ok(id)
    }
}

#[async_trait]
impl VectorStore for FakeStorage {
    async fn upsert_vectors(
        &self,
        vectors: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> post_cortex_ports::error::Result<()> {
        self.batch_calls.fetch_add(1, Ordering::Relaxed);
        for (v, m) in vectors {
            self.put(v, m)
                .map_err(post_cortex_ports::PortError::unavailable)?;
        }
        Ok(())
    }

    async fn delete_vectors(
        &self,
        session_id: &str,
        content_ids: &[String],
    ) -> post_cortex_ports::error::Result<usize> {
        let mut rows = self.rows.lock();
        Ok(content_ids
            .iter()
            .filter(|id| {
                rows.remove(&(session_id.to_string(), id.to_string()))
                    .is_some()
            })
            .count())
    }

    async fn has_session_embeddings(
        &self,
        session_id: &str,
    ) -> post_cortex_ports::error::Result<bool> {
        Ok(self.count_session_embeddings(session_id).await? > 0)
    }

    async fn count_session_embeddings(
        &self,
        session_id: &str,
    ) -> post_cortex_ports::error::Result<usize> {
        Ok(self
            .rows
            .lock()
            .keys()
            .filter(|(sid, _)| sid == session_id)
            .count())
    }

    async fn total_count(&self) -> post_cortex_ports::error::Result<usize> {
        Ok(self.len())
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> post_cortex_ports::error::Result<Vec<(Vec<f32>, VectorMetadata)>> {
        Ok(self
            .rows
            .lock()
            .iter()
            .filter(|((sid, _), _)| sid == session_id)
            .map(|(_, v)| v.clone())
            .collect())
    }

    async fn get_all_vectors(
        &self,
    ) -> post_cortex_ports::error::Result<Vec<(Vec<f32>, VectorMetadata)>> {
        Ok(self.rows.lock().values().cloned().collect())
    }
}

/// Scores a document by whether it contains `needle`; fails when `needle`
/// is empty.
pub(crate) struct FakeReranker {
    pub(crate) needle: &'static str,
}

#[async_trait]
impl Reranker for FakeReranker {
    async fn score(
        &self,
        _query: &str,
        documents: &[Arc<str>],
    ) -> post_cortex_ports::error::Result<Vec<f32>> {
        if self.needle.is_empty() {
            return Err(post_cortex_ports::PortError::unavailable("reranker down"));
        }
        Ok(documents
            .iter()
            .map(|d| if d.contains(self.needle) { 1.0 } else { 0.0 })
            .collect())
    }
}
