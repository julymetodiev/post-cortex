// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The in-memory [`SearchIndex`] adapter: the vector database
//! ([`VectorDB`]: a normalised arena with exact scans, BM25 postings) over
//! a storage adapter's persisted entries ([`VectorStore`]), written through
//! on every change. Vectors are L2-normalised before they are persisted.
//! The persisted entries are loaded on the first operation; entries of
//! another dimension than the model's are refused and counted, and a store
//! that holds only such entries fails the open. Every operation on the
//! database runs on the blocking pool, off the async runtime. The lexical
//! (BM25) ranking runs on the in-memory postings for every storage
//! backend.

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_ports::error::Result;
use post_cortex_ports::storage::VectorStore;
use post_cortex_ports::{
    EntrySelect, IndexEntry, IndexFilter, IndexHit, IndexStats, LoadReport, PortError, SearchIndex,
    VectorMetadata,
};
use tokio::sync::OnceCell;
use tracing::info;

use crate::vector_db::{VectorDB, normalize_in_place};

/// The in-memory index (see the module docs).
pub struct MemoryIndex {
    db: Arc<VectorDB>,
    store: Option<Arc<dyn VectorStore>>,
    opened: OnceCell<LoadReport>,
}

/// `job` over `db` on the blocking pool.
async fn off_runtime<T, F>(db: &Arc<VectorDB>, job: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&VectorDB) -> T + Send + 'static,
{
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || job(&db))
        .await
        .map_err(|e| PortError::Unavailable(format!("vector index task failed: {e}")))
}

impl MemoryIndex {
    /// An index of `dimension`-long vectors (the model's) over the entries
    /// of `store` (none: in memory only). Nothing is loaded until the first
    /// operation.
    pub fn new(dimension: usize, store: Option<Arc<dyn VectorStore>>) -> Self {
        Self {
            db: Arc::new(VectorDB::new(dimension)),
            store,
            opened: OnceCell::new(),
        }
    }

    /// Load the persisted entries once; a failed load is retried by the
    /// next operation.
    async fn open(&self) -> Result<&LoadReport> {
        self.opened.get_or_try_init(|| self.load()).await
    }

    async fn load(&self) -> Result<LoadReport> {
        let Some(store) = &self.store else {
            return Ok(LoadReport::default());
        };
        let vectors = store.get_all_vectors().await?;
        info!(
            "Loading {} embeddings from storage into memory",
            vectors.len()
        );
        if vectors.is_empty() {
            return Ok(LoadReport::default());
        }
        let report = off_runtime(&self.db, move |db| db.upsert(vectors)).await?;
        if report.loaded == 0 && report.rejected_dimension > 0 {
            return Err(PortError::Unavailable(format!(
                "every stored vector ({}) has another dimension than the model's {}: \
                 the store was written by another embedding model",
                report.rejected_dimension,
                self.db.dimension()
            )));
        }
        info!(
            "Successfully loaded {} embeddings into memory",
            report.loaded
        );
        Ok(LoadReport {
            loaded: report.loaded,
            rejected_dimension: report.rejected_dimension,
        })
    }

    fn check_dimension(&self, vector: &[f32], what: &str) -> Result<()> {
        if vector.len() == self.db.dimension() {
            return Ok(());
        }
        Err(PortError::Unavailable(format!(
            "{what} has {} dimensions, the index {}",
            vector.len(),
            self.db.dimension()
        )))
    }
}

#[async_trait]
impl SearchIndex for MemoryIndex {
    async fn upsert(&self, entries: Vec<IndexEntry>) -> Result<()> {
        self.open().await?;
        if entries.is_empty() {
            return Ok(());
        }
        for entry in &entries {
            self.check_dimension(&entry.vector, &format!("entry {}", entry.meta.id))?;
        }
        let items: Vec<(Vec<f32>, VectorMetadata)> = entries
            .into_iter()
            .map(|mut e| {
                normalize_in_place(&mut e.vector);
                (e.vector, e.meta)
            })
            .collect();
        let items = match &self.store {
            Some(store) => {
                store.upsert_vectors(items.clone()).await?;
                items
            }
            None => items,
        };
        off_runtime(&self.db, move |db| db.upsert(items)).await?;
        Ok(())
    }

    async fn delete(&self, session: &str, content_ids: &[String]) -> Result<usize> {
        self.open().await?;
        let (session_id, ids) = (session.to_string(), content_ids.to_vec());
        let (removed, stored) =
            off_runtime(&self.db, move |db| db.delete(&session_id, &ids)).await?;
        if let Some(store) = &self.store
            && !stored.is_empty()
        {
            store.delete_vectors(session, &stored).await?;
        }
        Ok(removed)
    }

    async fn entries(&self, session: &str, select: &EntrySelect) -> Result<Vec<IndexEntry>> {
        self.open().await?;
        let (session, select) = (session.to_string(), select.clone());
        off_runtime(&self.db, move |db| db.entries(&session, &select)).await
    }

    async fn nearest(
        &self,
        query: &[f32],
        filter: &IndexFilter,
        k: usize,
    ) -> Result<Vec<IndexHit>> {
        self.open().await?;
        self.check_dimension(query, "the query vector")?;
        let (query, filter) = (query.to_vec(), filter.clone());
        off_runtime(&self.db, move |db| db.nearest(&query, &filter, k)).await
    }

    async fn score(
        &self,
        query: &[f32],
        session: &str,
        content_ids: &[String],
    ) -> Result<Vec<IndexHit>> {
        self.open().await?;
        self.check_dimension(query, "the query vector")?;
        let (query, session, ids) = (query.to_vec(), session.to_string(), content_ids.to_vec());
        off_runtime(&self.db, move |db| db.score(&query, &session, &ids)).await
    }

    async fn lexical(&self, query: &str, filter: &IndexFilter, k: usize) -> Result<Vec<IndexHit>> {
        self.open().await?;
        let terms = self.db.query_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let filter = filter.clone();
        off_runtime(&self.db, move |db| db.lexical(&terms, &filter, k)).await
    }

    async fn stats(&self) -> Result<IndexStats> {
        let load = self.open().await?.clone();
        Ok(IndexStats {
            entries: self.db.len(),
            per_session: self.db.per_session(),
            dimension: self.db.dimension(),
            memory_bytes: self.db.memory_bytes(),
            load,
        })
    }
}

#[cfg(test)]
mod tests;
