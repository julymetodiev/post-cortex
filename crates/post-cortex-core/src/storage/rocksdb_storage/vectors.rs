// Copyright (c) 2025, 2026 Julius ML
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

//! Embedding persistence and the [`VectorStorage`] trait implementation
//! (in-memory HNSW index backed by RocksDB-persisted [`StoredEmbedding`] rows).

use anyhow::Result;
use async_trait::async_trait;
use rocksdb::WriteBatch;
use tracing::debug;

use post_cortex_embeddings::{SearchMatch, VectorMetadata};
use crate::storage::traits::VectorStorage;

use super::RealRocksDBStorage;
use super::types::{EMBEDDING_DIMENSION, StoredEmbedding};

impl RealRocksDBStorage {
    /// Save an embedding to RocksDB
    pub async fn save_embedding(&self, embedding: &StoredEmbedding) -> Result<()> {
        let db = self.db.clone();
        let embedding = embedding.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = format!(
                "embedding:{}:{}",
                embedding.session_id, embedding.content_id
            );
            let data = bincode::serde::encode_to_vec(&embedding, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize embedding: {}", e))?;
            db.put(key.as_bytes(), &data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load all embeddings for a session
    pub async fn load_session_embeddings(&self, session_id: &str) -> Result<Vec<StoredEmbedding>> {
        let db = self.db.clone();
        let prefix = format!("embedding:{}:", session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredEmbedding>> {
            let mut embeddings = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&prefix) {
                    break;
                }

                if let Ok((embedding, _)) = bincode::serde::decode_from_slice::<StoredEmbedding, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    embeddings.push(embedding);
                }
            }

            Ok(embeddings)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Load all embeddings from storage
    /// Used for HNSW index rebuild and migration operations
    pub async fn load_all_embeddings(&self) -> Result<Vec<StoredEmbedding>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredEmbedding>> {
            let mut embeddings = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"embedding:",
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with("embedding:") {
                    break;
                }

                if let Ok((embedding, _)) = bincode::serde::decode_from_slice::<StoredEmbedding, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    embeddings.push(embedding);
                }
            }

            Ok(embeddings)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Delete an embedding
    pub async fn delete_embedding(&self, session_id: &str, content_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = format!("embedding:{}:{}", session_id, content_id);

        tokio::task::spawn_blocking(move || -> Result<bool> {
            let existed = db.get(key.as_bytes())?.is_some();
            db.delete(key.as_bytes())?;
            Ok(existed)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Count embeddings for a session
    pub async fn count_embeddings(&self, session_id: &str) -> usize {
        self.load_session_embeddings(session_id)
            .await
            .map(|e| e.len())
            .unwrap_or(0)
    }

    /// Batch save embeddings in a single RocksDB WriteBatch.
    pub async fn batch_save_embeddings(&self, embeddings: &[StoredEmbedding]) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }

        let db = self.db.clone();
        let embeddings = embeddings.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            for embedding in &embeddings {
                let key = format!(
                    "embedding:{}:{}",
                    embedding.session_id, embedding.content_id
                );
                let data = bincode::serde::encode_to_vec(embedding, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize embedding: {}", e))?;
                batch.put(key.as_bytes(), &data);
            }

            db.write(batch)?;
            debug!("Batch saved {} embeddings", embeddings.len());
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }
}

#[async_trait]
impl VectorStorage for RealRocksDBStorage {
    async fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String> {
        // Validate embedding dimension for consistency with SurrealDB backend
        if vector.len() != EMBEDDING_DIMENSION {
            return Err(anyhow::anyhow!(
                "Invalid embedding dimension: expected {}, got {}",
                EMBEDDING_DIMENSION,
                vector.len()
            ));
        }
        let id = metadata.id.clone();

        // Add to in-memory HNSW index for fast search
        self.vector_index
            .add_vector(vector.clone(), metadata.clone())?;

        // Persist to RocksDB for durability
        let embedding = StoredEmbedding::new(vector, metadata);
        self.save_embedding(&embedding).await?;

        Ok(id)
    }

    async fn add_vectors_batch(
        &self,
        vectors: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for (vector, metadata) in vectors {
            let id = self.add_vector(vector, metadata).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    /// Search using HNSW index - O(log n) complexity
    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchMatch>> {
        // Use HNSW index for fast approximate nearest neighbor search
        self.vector_index.search(query, k)
    }

    /// Search within a session using HNSW with post-filtering
    async fn search_in_session(
        &self,
        query: &[f32],
        k: usize,
        session_id: &str,
    ) -> Result<Vec<SearchMatch>> {
        // Use HNSW with post-filtering: fetch more results, filter by session
        // This matches SurrealDB's approach for filtered queries
        let fetch_multiplier = 5;
        let results = self.vector_index.search(query, k * fetch_multiplier)?;

        // Filter by session and take top k
        let filtered: Vec<SearchMatch> = results
            .into_iter()
            .filter(|m| m.metadata.source == session_id)
            .take(k)
            .collect();

        Ok(filtered)
    }

    /// Search by content type using HNSW with post-filtering
    async fn search_by_content_type(
        &self,
        query: &[f32],
        k: usize,
        content_type: &str,
    ) -> Result<Vec<SearchMatch>> {
        // Use HNSW with post-filtering by content type
        let fetch_multiplier = 5;
        let results = self.vector_index.search(query, k * fetch_multiplier)?;

        // Filter by content type and take top k
        let filtered: Vec<SearchMatch> = results
            .into_iter()
            .filter(|m| m.metadata.content_type == content_type)
            .take(k)
            .collect();

        Ok(filtered)
    }

    async fn remove_vector(&self, id: &str) -> Result<bool> {
        // Find vector_id in HNSW index by content_id and remove it
        let mut removed = false;
        let vector_id = self.vector_index.find_vector_id_by_content_id(id);

        if let Some(vid) = vector_id {
            self.vector_index.remove_vector(vid)?;
            removed = true;
        }

        // Remove from RocksDB
        let embeddings = self.load_all_embeddings().await?;
        for e in embeddings {
            if e.content_id == id {
                self.delete_embedding(&e.session_id, &e.content_id).await?;
                return Ok(true);
            }
        }

        Ok(removed)
    }

    async fn has_session_embeddings(&self, session_id: &str) -> bool {
        self.count_embeddings(session_id).await > 0
    }

    async fn count_session_embeddings(&self, session_id: &str) -> usize {
        self.count_embeddings(session_id).await
    }

    async fn total_count(&self) -> usize {
        self.vector_index.len()
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let embeddings = self.load_session_embeddings(session_id).await?;
        Ok(embeddings
            .into_iter()
            .map(|e| {
                let metadata = e.to_metadata();
                (e.vector, metadata)
            })
            .collect())
    }

    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let embeddings = self.load_all_embeddings().await?;
        Ok(embeddings
            .into_iter()
            .map(|e| {
                let metadata = e.to_metadata();
                (e.vector, metadata)
            })
            .collect())
    }
}
