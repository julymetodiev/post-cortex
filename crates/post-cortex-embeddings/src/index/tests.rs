// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The in-memory index: the port's conformance suite in memory only, and
//! its open over persisted entries (write-through, dimension check).

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use post_cortex_ports::conformance::index::{DIMENSION, entry};
use post_cortex_ports::error::Result;
use post_cortex_ports::storage::VectorStore;
use post_cortex_ports::{EntrySelect, SearchIndex, VectorMetadata};

use crate::index::MemoryIndex;

async fn in_memory() -> (Arc<dyn SearchIndex>, ()) {
    (Arc::new(MemoryIndex::new(DIMENSION, None)), ())
}

mod conformance {
    post_cortex_ports::index_conformance_suite!(crate::index::tests::in_memory());
}

/// Persisted vectors in a list.
#[derive(Default)]
struct ListStore(Mutex<Vec<(Vec<f32>, VectorMetadata)>>);

#[async_trait]
impl VectorStore for ListStore {
    async fn upsert_vectors(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> Result<()> {
        let mut list = self.0.lock();
        for (v, m) in vectors {
            list.retain(|(_, s)| !(s.id == m.id && s.source == m.source));
            list.push((v, m));
        }
        Ok(())
    }
    async fn delete_vectors(&self, session_id: &str, content_ids: &[String]) -> Result<usize> {
        let mut list = self.0.lock();
        let before = list.len();
        list.retain(|(_, m)| !(m.source == session_id && content_ids.contains(&m.id)));
        Ok(before - list.len())
    }
    async fn has_session_embeddings(&self, session_id: &str) -> Result<bool> {
        Ok(self.0.lock().iter().any(|(_, m)| m.source == session_id))
    }
    async fn count_session_embeddings(&self, session_id: &str) -> Result<usize> {
        Ok(self
            .0
            .lock()
            .iter()
            .filter(|(_, m)| m.source == session_id)
            .count())
    }
    async fn total_count(&self) -> Result<usize> {
        Ok(self.0.lock().len())
    }
    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        Ok(self
            .0
            .lock()
            .iter()
            .filter(|(_, m)| m.source == session_id)
            .cloned()
            .collect())
    }
    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        Ok(self.0.lock().clone())
    }
}

#[tokio::test]
async fn changes_are_written_through_and_loaded_at_open() {
    let store = Arc::new(ListStore::default());
    let first = MemoryIndex::new(DIMENSION, Some(store.clone()));
    first
        .upsert(vec![
            entry("a", "u1", 1, "one"),
            entry("a", "u1#2", 2, "two"),
        ])
        .await
        .unwrap();
    first
        .upsert(vec![entry("a", "u2", 3, "three")])
        .await
        .unwrap();
    first.delete("a", &["u2".to_string()]).await.unwrap();
    assert_eq!(store.0.lock().len(), 2);

    let reopened = MemoryIndex::new(DIMENSION, Some(store));
    let stats = reopened.stats().await.unwrap();
    assert_eq!(stats.load.loaded, 2);
    assert_eq!(stats.entries, 2);
    let entries = reopened.entries("a", &EntrySelect::All).await.unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test]
async fn the_open_refuses_another_models_vectors() {
    let store = Arc::new(ListStore::default());
    let mut other = entry("a", "u1", 1, "one");
    other.vector.push(0.5);
    store
        .upsert_vectors(vec![(other.vector, other.meta)])
        .await
        .unwrap();

    let index = MemoryIndex::new(DIMENSION, Some(store.clone()));
    let err = index.stats().await.unwrap_err();
    assert!(err.to_string().contains("another embedding model"), "{err}");

    let ours = entry("a", "u2", 2, "two");
    store
        .upsert_vectors(vec![(ours.vector, ours.meta)])
        .await
        .unwrap();
    let stats = index.stats().await.unwrap();
    assert_eq!((stats.load.loaded, stats.load.rejected_dimension), (1, 1));
}
