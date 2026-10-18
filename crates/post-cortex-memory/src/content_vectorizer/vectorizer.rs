// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! `ContentVectorizer` — the orchestrator type plus initialization and persistent-storage lifecycle.

use anyhow::{Context, Result};
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::query_cache::QueryCache;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::LocalEmbeddingEngine;
use post_cortex_embeddings::{VectorDB, VectorMetadata};

use super::types::ContentVectorizerConfig;

/// Main content vectorization pipeline
pub struct ContentVectorizer {
    pub(super) embedding_engine: Arc<LocalEmbeddingEngine>,
    pub(super) vector_db: Arc<VectorDB>,
    pub(super) config: ContentVectorizerConfig,
    pub(super) query_cache: Option<Arc<QueryCache>>,
    /// Optional persistent storage for embeddings (RocksDB/SurrealDB)
    pub(super) persistent_storage: Option<Arc<dyn post_cortex_storage::traits::VectorStorage>>,
    /// Recency bias performance metrics — all atomic for lock-free access
    pub(super) recency_bias_total_duration_ns: Arc<AtomicU64>,
    pub(super) recency_bias_total_results: Arc<AtomicU64>,
    pub(super) recency_bias_calculation_count: Arc<AtomicU64>,
    /// Per session: the (update count, entity count) at the last complete
    /// `vectorize_session`. Lets searches skip re-vectorizing a session
    /// that has nothing new — including sessions whose updates are all too
    /// short to embed, which never get update vectors.
    pub(super) vectorized_marks: Arc<DashMap<Uuid, (usize, usize)>>,
}

impl Clone for ContentVectorizer {
    fn clone(&self) -> Self {
        Self {
            embedding_engine: Arc::clone(&self.embedding_engine),
            vector_db: Arc::clone(&self.vector_db),
            config: self.config.clone(),
            query_cache: self.query_cache.clone(),
            persistent_storage: self.persistent_storage.clone(),
            // Atomics are shared between clones via Arc for metrics aggregation
            recency_bias_total_duration_ns: Arc::clone(&self.recency_bias_total_duration_ns),
            recency_bias_total_results: Arc::clone(&self.recency_bias_total_results),
            recency_bias_calculation_count: Arc::clone(&self.recency_bias_calculation_count),
            vectorized_marks: Arc::clone(&self.vectorized_marks),
        }
    }
}

impl ContentVectorizer {
    /// Create a new `ContentVectorizer` with the given configuration.
    ///
    /// # Errors
    /// Returns an error if the vector database or embedding engine fails to initialize.
    pub async fn new(config: ContentVectorizerConfig) -> Result<Self> {
        info!(
            "Initializing ContentVectorizer with caching: {}",
            config.enable_query_caching
        );

        let embedding_engine = LocalEmbeddingEngine::new(config.embedding_config.clone())
            .await
            .context("Failed to initialize embedding engine")?;

        let vector_db = VectorDB::new(config.vector_db_config.clone())
            .context("Failed to initialize vector database")?;

        let query_cache = if config.enable_query_caching {
            Some(QueryCache::new(config.query_cache_config.clone()))
        } else {
            None
        };

        debug!(
            "ContentVectorizer initialized successfully with query caching: {}",
            config.enable_query_caching
        );

        Ok(Self {
            embedding_engine: Arc::new(embedding_engine),
            vector_db: Arc::new(vector_db),
            config,
            query_cache: query_cache.map(Arc::new),
            persistent_storage: None,
            recency_bias_total_duration_ns: Arc::new(AtomicU64::new(0)),
            recency_bias_total_results: Arc::new(AtomicU64::new(0)),
            recency_bias_calculation_count: Arc::new(AtomicU64::new(0)),
            vectorized_marks: Arc::new(DashMap::new()),
        })
    }

    /// Set persistent storage for embedding persistence.
    /// Embeddings will be saved to this storage when added.
    pub fn with_persistent_storage(
        mut self,
        storage: Arc<dyn post_cortex_storage::traits::VectorStorage>,
    ) -> Self {
        self.persistent_storage = Some(storage);
        self
    }

    /// Set persistent storage (mutable version)
    pub fn set_persistent_storage(
        &mut self,
        storage: Arc<dyn post_cortex_storage::traits::VectorStorage>,
    ) {
        self.persistent_storage = Some(storage);
    }

    /// Add a vector to the in-memory DB **and** persist it. Returns `Ok(true)` only when
    /// both succeed — matches the existing semantics: an in-memory add that fails to
    /// persist must NOT be marked as vectorized (so a retry will pick it up next time).
    /// A failed persist also rolls the in-memory add back, so the retry does not
    /// leave a duplicate behind.
    pub(super) async fn add_and_persist(
        &self,
        embedding: Vec<f32>,
        metadata: VectorMetadata,
    ) -> bool {
        !self
            .add_and_persist_many(vec![(embedding, metadata)])
            .await
            .is_empty()
    }

    /// Batch form of [`Self::add_and_persist`]: in-memory adds, then one
    /// storage batch write. Returns the content ids that were both added and
    /// persisted. The in-memory DB takes ownership of each vector; only the
    /// copy handed to storage is cloned.
    pub(super) async fn add_and_persist_many(
        &self,
        items: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> Vec<String> {
        let mut added_ids = Vec::with_capacity(items.len());
        let mut content_ids = Vec::with_capacity(items.len());
        let mut to_persist = Vec::new();

        for (embedding, metadata) in items {
            let content_id = metadata.id.clone();
            let persist_copy = self
                .persistent_storage
                .as_ref()
                .map(|_| (embedding.clone(), metadata.clone()));
            match self.vector_db.add_vector(embedding, metadata) {
                Ok(vector_id) => {
                    added_ids.push(vector_id);
                    content_ids.push(content_id);
                    to_persist.extend(persist_copy);
                }
                Err(e) => warn!("Failed to add vector {} to database: {}", content_id, e),
            }
        }

        if let Some(storage) = &self.persistent_storage
            && !to_persist.is_empty()
        {
            let persisted = if to_persist.len() == 1 {
                let (vector, metadata) = to_persist.pop().expect("one item");
                storage.add_vector(vector, metadata).await.map(|_| ())
            } else {
                storage.add_vectors_batch(to_persist).await.map(|_| ())
            };
            if let Err(e) = persisted {
                warn!(
                    "Failed to persist {} vector(s) ({}): {:#}. NOT marking as vectorized.",
                    content_ids.len(),
                    content_ids.join(", "),
                    e
                );
                self.vector_db.remove_vectors(&added_ids);
                return Vec::new();
            }
        }

        content_ids
    }

    /// Drop cached query results that a change to `session_id`'s vectors
    /// can affect (its own, multi-session and global entries).
    pub(super) fn invalidate_cached(&self, session_id: Uuid) {
        if let Some(cache) = &self.query_cache
            && let Err(e) = cache.invalidate_session(session_id)
        {
            debug!("Query cache invalidation for {} failed: {}", session_id, e);
        }
    }

    /// Whether `vectorize_session` has work to do for this session: false
    /// only when the last complete run saw the same update and entity counts.
    pub fn needs_vectorization(&self, session: &ActiveSession) -> bool {
        self.vectorized_marks
            .get(&session.id())
            .is_none_or(|mark| *mark != vectorization_mark(session))
    }

    /// Remember that `session` (this snapshot) is fully vectorized.
    pub(super) fn mark_vectorized(&self, session: &ActiveSession) {
        self.vectorized_marks
            .insert(session.id(), vectorization_mark(session));
    }

    /// Remove every in-memory vector of a session (e.g. on session delete)
    /// and invalidate the query cache entries it affects. Persisted
    /// embeddings are left to the storage layer's session delete. Returns
    /// how many vectors were removed.
    pub fn remove_session_vectors(&self, session_id: Uuid) -> usize {
        let removed = self.vector_db.remove_source(&session_id.to_string());
        self.vectorized_marks.remove(&session_id);
        self.invalidate_cached(session_id);
        debug!("Removed {} vectors of session {}", removed, session_id);
        removed
    }

    /// Remove the vectors of one content item (an update id, or
    /// `entity:<name>`) belonging to `session_id`, from memory and from
    /// persistent storage, and invalidate affected cached queries.
    ///
    /// Only that session's in-memory copies are touched: entity content ids
    /// are not unique across sessions. Returns how many in-memory vectors
    /// were removed.
    ///
    /// # Errors
    /// Returns an error if the persisted embedding could not be deleted.
    pub async fn remove_content(&self, session_id: Uuid, content_id: &str) -> Result<usize> {
        let ids = source_content_ids(&self.vector_db, content_id, &session_id.to_string());
        let removed = self.vector_db.remove_vectors(&ids);
        self.invalidate_cached(session_id);

        if let Some(storage) = &self.persistent_storage {
            // Session-scoped: entity content ids (`entity:<name>`) repeat
            // across sessions, and other sessions' copies must stay.
            storage
                .remove_session_vector(&session_id.to_string(), content_id)
                .await
                .with_context(|| format!("Failed to delete persisted embedding {content_id}"))?;
        }
        Ok(removed)
    }

    /// Load embeddings from persistent storage into the in-memory vector database.
    /// Call this after creating the vectorizer to restore persisted embeddings.
    pub async fn load_embeddings_from_storage(&self, session_id: &str) -> Result<usize> {
        let storage = match &self.persistent_storage {
            Some(s) => s,
            None => {
                debug!("No persistent storage configured, skipping embedding load");
                return Ok(0);
            }
        };

        if !storage.has_session_embeddings(session_id).await {
            debug!("No embeddings found in storage for session {}", session_id);
            return Ok(0);
        }

        let vectors = storage.get_session_vectors(session_id).await?;
        if vectors.is_empty() {
            debug!(
                "No vectors returned from storage for session {}",
                session_id
            );
            return Ok(0);
        }

        info!(
            "Loading {} embeddings from storage for session {}",
            vectors.len(),
            session_id
        );
        let loaded = self.load_vectors_into_db(vectors).await;
        info!(
            "Successfully loaded {} embeddings into memory for session {}",
            loaded, session_id
        );
        Ok(loaded)
    }

    /// Load all embeddings from persistent storage into the in-memory vector database.
    /// Call this on startup to restore all persisted embeddings.
    pub async fn load_all_embeddings_from_storage(&self) -> Result<usize> {
        let storage = match &self.persistent_storage {
            Some(s) => s,
            None => {
                debug!("No persistent storage configured, skipping embedding load");
                return Ok(0);
            }
        };

        let vectors = storage.get_all_vectors().await?;
        if vectors.is_empty() {
            debug!("No vectors found in storage");
            return Ok(0);
        }

        info!(
            "Loading {} embeddings from storage into memory",
            vectors.len()
        );
        let loaded = self.load_vectors_into_db(vectors).await;
        info!("Successfully loaded {} embeddings into memory", loaded);
        Ok(loaded)
    }

    /// Bulk-load vectors on a blocking thread (one HNSW build at the end),
    /// skipping content ids that are already in memory so a repeated load
    /// does not duplicate them.
    async fn load_vectors_into_db(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> usize {
        let db = Arc::clone(&self.vector_db);
        let result = tokio::task::spawn_blocking(move || {
            if db.is_empty() {
                return db.add_vectors_bulk(vectors);
            }
            let fresh = vectors
                .into_iter()
                .filter(|(_, metadata)| {
                    source_content_ids(&db, &metadata.id, &metadata.source).is_empty()
                })
                .collect::<Vec<_>>();
            db.add_vectors_bulk(fresh)
        })
        .await;
        match result {
            Ok(Ok(ids)) => ids.len(),
            Ok(Err(e)) => {
                warn!("Failed to load vectors into memory: {}", e);
                0
            }
            Err(e) => {
                warn!("Vector load task failed: {}", e);
                0
            }
        }
    }

    /// Check if persistent storage has embeddings for a session.
    pub async fn has_persisted_embeddings(&self, session_id: &str) -> bool {
        if let Some(storage) = &self.persistent_storage {
            storage.has_session_embeddings(session_id).await
        } else {
            false
        }
    }

    /// Get count of persisted embeddings for a session.
    pub async fn count_persisted_embeddings(&self, session_id: &str) -> usize {
        if let Some(storage) = &self.persistent_storage {
            storage.count_session_embeddings(session_id).await
        } else {
            0
        }
    }
}

/// Snapshot of what a complete `vectorize_session` covered.
fn vectorization_mark(session: &ActiveSession) -> (usize, usize) {
    (
        session.incremental_updates.len(),
        session.entity_graph.entity_count(),
    )
}

/// Vector ids of `content_id` whose source is `source`. Entity content ids
/// (`entity:<name>`) repeat across sessions, so the content-id index alone
/// is not enough. O(copies of that content id).
pub(super) fn source_content_ids(db: &VectorDB, content_id: &str, source: &str) -> Vec<u32> {
    db.find_vector_ids_by_content_id(content_id)
        .into_iter()
        .filter(|&id| db.get_metadata(id).is_some_and(|m| m.source == source))
        .collect()
}
