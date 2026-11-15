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

//! Embedding persistence: the [`VectorStore`] port on RocksDB.
//!
//! Key families:
//! - `embedding:{session_id}:{content_id}` -> marked bincode
//!   [`StoredEmbedding`] (see [`crate::rocksdb_storage::rows`]).
//!   Content ids repeat across sessions (`entity:<name>`), so the session
//!   is part of the key and a write of the same content replaces its row.
//!
//! Search runs on the memory layer's in-memory index; this backend only
//! persists embeddings.

use crate::port_error::PortResult;
use crate::port_error::{AdapterError, Result};
use async_trait::async_trait;
use rocksdb::WriteBatch;
use tracing::debug;

use post_cortex_ports::VectorMetadata;
use post_cortex_ports::storage::VectorStore;

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, keys_with_prefix, scan_prefix};
use crate::rocksdb_storage::rows::{EMBEDDING_V1, decode_marked, encode_marked};
use crate::rocksdb_storage::types::StoredEmbedding;

const EMBEDDING_PREFIX: &str = "embedding:";

fn embedding_key(session_id: &str, content_id: &str) -> String {
    format!("embedding:{}:{}", session_id, content_id)
}

fn decode_embedding(key: &str, row: &[u8]) -> Result<StoredEmbedding> {
    decode_marked("embedding", key, EMBEDDING_V1, row)
}

/// Add the row to `batch`.
fn put_embedding(batch: &mut WriteBatch, embedding: &StoredEmbedding) -> Result<()> {
    batch.put(
        embedding_key(&embedding.session_id, &embedding.content_id).as_bytes(),
        encode_marked(EMBEDDING_V1, embedding)?,
    );
    Ok(())
}

impl RealRocksDBStorage {
    /// Reject vectors this storage was not configured for.
    fn validate_vector(&self, vector: &[f32]) -> Result<()> {
        match self.vector_dimension {
            Some(dim) if vector.len() != dim => Err(AdapterError::Dimension {
                expected: dim,
                got: vector.len(),
            }),
            None if vector.is_empty() => Err(AdapterError::TooShort { got: 0, min: 1 }),
            _ => Ok(()),
        }
    }

    /// The embeddings under `prefix`.
    async fn load_embeddings_with_prefix(&self, prefix: String) -> Result<Vec<StoredEmbedding>> {
        let db = self.db.clone();
        blocking(move || {
            scan_prefix(&db, prefix.as_bytes())
                .map(|item| {
                    let (key, value) = item?;
                    decode_embedding(&String::from_utf8_lossy(&key), &value)
                })
                .collect()
        })
        .await
    }
}

/// `(vector, metadata)` pairs of stored embeddings.
fn into_vectors(embeddings: Vec<StoredEmbedding>) -> Vec<(Vec<f32>, VectorMetadata)> {
    embeddings
        .into_iter()
        .map(|e| {
            let metadata = e.to_metadata();
            (e.vector, metadata)
        })
        .collect()
}

#[async_trait]
impl VectorStore for RealRocksDBStorage {
    async fn upsert_vectors(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> PortResult<()> {
        self.breaker
            .guard(async {
                if vectors.is_empty() {
                    return Ok(());
                }
                let mut batch = WriteBatch::default();
                for (vector, metadata) in vectors {
                    self.validate_vector(&vector)?;
                    put_embedding(&mut batch, &StoredEmbedding::new(vector, metadata))?;
                }
                let db = self.db.clone();
                blocking(move || {
                    let count = batch.len();
                    db.write(batch)?;
                    debug!("Stored {} embeddings", count);
                    Ok(())
                })
                .await
            })
            .await
    }

    async fn delete_vectors(&self, session_id: &str, content_ids: &[String]) -> PortResult<usize> {
        self.breaker
            .guard(async {
                if content_ids.is_empty() {
                    return Ok(0);
                }
                let db = self.db.clone();
                let keys: Vec<String> = content_ids
                    .iter()
                    .map(|id| embedding_key(session_id, id))
                    .collect();
                blocking(move || {
                    let mut batch = WriteBatch::default();
                    let mut existed = 0;
                    for (key, value) in keys.iter().zip(db.multi_get(&keys)) {
                        if value?.is_some() {
                            existed += 1;
                            batch.delete(key.as_bytes());
                        }
                    }
                    db.write(batch)?;
                    Ok(existed)
                })
                .await
            })
            .await
    }

    async fn has_session_embeddings(&self, session_id: &str) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let prefix = format!("embedding:{}:", session_id);
                blocking(move || {
                    Ok(scan_prefix(&db, prefix.as_bytes())
                        .next()
                        .transpose()?
                        .is_some())
                })
                .await
            })
            .await
    }

    /// Keys only, no decoding.
    async fn count_session_embeddings(&self, session_id: &str) -> PortResult<usize> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let prefix = format!("embedding:{}:", session_id);
                blocking(move || Ok(keys_with_prefix(&db, prefix.as_bytes())?.len())).await
            })
            .await
    }

    async fn total_count(&self) -> PortResult<usize> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || Ok(keys_with_prefix(&db, EMBEDDING_PREFIX.as_bytes())?.len()))
                    .await
            })
            .await
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> PortResult<Vec<(Vec<f32>, VectorMetadata)>> {
        self.breaker
            .guard(async {
                let embeddings = self
                    .load_embeddings_with_prefix(format!("embedding:{}:", session_id))
                    .await?;
                Ok(into_vectors(embeddings))
            })
            .await
    }

    async fn get_all_vectors(&self) -> PortResult<Vec<(Vec<f32>, VectorMetadata)>> {
        self.breaker
            .guard(async {
                let embeddings = self
                    .load_embeddings_with_prefix(EMBEDDING_PREFIX.to_string())
                    .await?;
                Ok(into_vectors(embeddings))
            })
            .await
    }
}
