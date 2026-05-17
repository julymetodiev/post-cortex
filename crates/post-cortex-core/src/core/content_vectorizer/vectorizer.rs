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
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tracing::{debug, info, warn};

use crate::core::embeddings::LocalEmbeddingEngine;
use crate::core::query_cache::QueryCache;
use crate::core::vector_db::{VectorDB, VectorMetadata};

use super::types::ContentVectorizerConfig;

/// Main content vectorization pipeline
pub struct ContentVectorizer {
    pub(super) embedding_engine: Arc<LocalEmbeddingEngine>,
    pub(super) vector_db: Arc<VectorDB>,
    pub(super) config: ContentVectorizerConfig,
    pub(super) query_cache: Option<Arc<QueryCache>>,
    /// Optional persistent storage for embeddings (RocksDB/SurrealDB)
    pub(super) persistent_storage: Option<Arc<dyn crate::storage::traits::VectorStorage>>,
    /// Recency bias performance metrics — all atomic for lock-free access
    pub(super) recency_bias_total_duration_ns: Arc<AtomicU64>,
    pub(super) recency_bias_total_results: Arc<AtomicU64>,
    pub(super) recency_bias_calculation_count: Arc<AtomicU64>,
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
        })
    }

    /// Set persistent storage for embedding persistence.
    /// Embeddings will be saved to this storage when added.
    pub fn with_persistent_storage(
        mut self,
        storage: Arc<dyn crate::storage::traits::VectorStorage>,
    ) -> Self {
        self.persistent_storage = Some(storage);
        self
    }

    /// Set persistent storage (mutable version)
    pub fn set_persistent_storage(
        &mut self,
        storage: Arc<dyn crate::storage::traits::VectorStorage>,
    ) {
        self.persistent_storage = Some(storage);
    }

    /// Persist a vector to storage if configured.
    pub(super) async fn persist_vector(
        &self,
        vector: Vec<f32>,
        metadata: VectorMetadata,
    ) -> Result<()> {
        if let Some(storage) = &self.persistent_storage {
            storage
                .add_vector(vector, metadata.clone())
                .await
                .context(format!("Failed to persist embedding {}", metadata.id))?;
        }
        Ok(())
    }

    /// Add a vector to the in-memory DB **and** persist it. Returns `Ok(true)` only when
    /// both succeed — matches the existing semantics: an in-memory add that fails to
    /// persist must NOT be marked as vectorized (so a retry will pick it up next time).
    pub(super) async fn add_and_persist(
        &self,
        embedding: Vec<f32>,
        metadata: VectorMetadata,
    ) -> bool {
        match self.vector_db.add_vector(embedding.clone(), metadata.clone()) {
            Ok(_) => match self.persist_vector(embedding, metadata.clone()).await {
                Ok(_) => true,
                Err(e) => {
                    warn!(
                        "Failed to persist vector {}: {}. NOT marking as vectorized.",
                        metadata.id, e
                    );
                    false
                }
            },
            Err(e) => {
                warn!("Failed to add vector {} to database: {}", metadata.id, e);
                false
            }
        }
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
            debug!("No vectors returned from storage for session {}", session_id);
            return Ok(0);
        }

        info!(
            "Loading {} embeddings from storage for session {}",
            vectors.len(),
            session_id
        );
        let loaded = self.load_vectors_into_db(vectors);
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

        info!("Loading {} embeddings from storage into memory", vectors.len());
        let loaded = self.load_vectors_into_db(vectors);
        info!("Successfully loaded {} embeddings into memory", loaded);
        Ok(loaded)
    }

    fn load_vectors_into_db(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> usize {
        let mut loaded = 0;
        for (vector, metadata) in vectors {
            match self.vector_db.add_vector(vector, metadata.clone()) {
                Ok(_) => loaded += 1,
                Err(e) => warn!("Failed to load vector {} into memory: {}", metadata.id, e),
            }
        }
        loaded
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
