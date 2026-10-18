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

//! Embedding persistence and the [`VectorStorage`] trait implementation.
//!
//! Key families:
//! - `embedding:{session_id}:{content_id}` -> bincode [`StoredEmbedding`]
//! - `embedding_cid:{content_id}` -> session id (lookup for removal by
//!   content id; rows written before this index existed are found by a
//!   key scan instead)
//!
//! The memory layer keeps its own in-memory vector index and only uses this
//! backend for persistence, so the search methods here are exact scans over
//! the persisted rows rather than a second resident index.

use anyhow::Result;
use async_trait::async_trait;
use rocksdb::{DB, WriteBatch};
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use tracing::debug;

use crate::traits::VectorStorage;
use post_cortex_embeddings::{SearchMatch, VectorMetadata, cosine_with_norms, l2_norm};

use super::RealRocksDBStorage;
use super::core::{blocking, keys_with_prefix, scan_prefix};
use super::types::StoredEmbedding;

const EMBEDDING_PREFIX: &str = "embedding:";

fn embedding_key(session_id: &str, content_id: &str) -> String {
    format!("embedding:{}:{}", session_id, content_id)
}

fn content_index_key(content_id: &str) -> String {
    format!("embedding_cid:{}", content_id)
}

fn decode_embedding(value: &[u8]) -> Option<StoredEmbedding> {
    bincode::serde::decode_from_slice::<StoredEmbedding, _>(value, bincode::config::standard())
        .ok()
        .map(|(e, _)| e)
}

/// Add the row and its content-id index entry to `batch`.
fn put_embedding(batch: &mut WriteBatch, embedding: &StoredEmbedding) -> Result<()> {
    let data = bincode::serde::encode_to_vec(embedding, bincode::config::standard())
        .map_err(|e| anyhow::anyhow!("Failed to serialize embedding: {}", e))?;
    batch.put(
        embedding_key(&embedding.session_id, &embedding.content_id).as_bytes(),
        &data,
    );
    batch.put(
        content_index_key(&embedding.content_id).as_bytes(),
        embedding.session_id.as_bytes(),
    );
    Ok(())
}

/// Delete every embedding row for `content_id` (normally one) plus its index
/// entry. Returns whether any row existed.
fn delete_by_content_id(db: &DB, content_id: &str) -> Result<bool> {
    let index_key = content_index_key(content_id);
    let mut row_keys: Vec<Vec<u8>> = Vec::new();

    if let Some(session_id) = db.get(index_key.as_bytes())? {
        let key = embedding_key(&String::from_utf8_lossy(&session_id), content_id);
        if db.get(key.as_bytes())?.is_some() {
            row_keys.push(key.into_bytes());
        }
    } else {
        // Rows persisted before the index existed: match on the key alone
        // (`embedding:{uuid}:{content_id}`; session ids contain no ':').
        for key in keys_with_prefix(db, EMBEDDING_PREFIX.as_bytes())? {
            let rest = &key[EMBEDDING_PREFIX.len()..];
            if let Some(pos) = rest.iter().position(|b| *b == b':')
                && &rest[pos + 1..] == content_id.as_bytes()
            {
                row_keys.push(key.into_vec());
            }
        }
    }

    let mut batch = WriteBatch::default();
    batch.delete(index_key.as_bytes());
    for key in &row_keys {
        batch.delete(key);
    }
    db.write(batch)?;
    Ok(!row_keys.is_empty())
}

/// Score candidate with a total order (NaN sorts lowest) for the top-k heap.
struct Scored {
    similarity: f32,
    ordinal: u32,
    embedding: StoredEmbedding,
}

impl PartialEq for Scored {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scored {
    fn cmp(&self, other: &Self) -> Ordering {
        self.similarity
            .total_cmp(&other.similarity)
            .then_with(|| other.ordinal.cmp(&self.ordinal))
    }
}

/// Exact top-`k` cosine search over the rows under `prefix` that pass
/// `filter`. `SearchMatch::vector_id` is the row's ordinal in the scan.
fn scan_search(
    db: &DB,
    prefix: &str,
    query: &[f32],
    k: usize,
    filter: impl Fn(&StoredEmbedding) -> bool,
) -> Result<Vec<SearchMatch>> {
    if k == 0 {
        return Ok(Vec::new());
    }
    let query_norm = l2_norm(query);
    let mut heap: BinaryHeap<Reverse<Scored>> = BinaryHeap::with_capacity(k + 1);

    for (ordinal, item) in scan_prefix(db, prefix.as_bytes()).enumerate() {
        let (_, value) = item?;
        let Some(embedding) = decode_embedding(&value) else {
            continue;
        };
        if embedding.vector.len() != query.len() || !filter(&embedding) {
            continue;
        }
        let similarity = cosine_with_norms(
            query,
            &embedding.vector,
            query_norm,
            l2_norm(&embedding.vector),
        );
        let ordinal = u32::try_from(ordinal).unwrap_or(u32::MAX);
        if heap.len() < k {
            heap.push(Reverse(Scored {
                similarity,
                ordinal,
                embedding,
            }));
        } else if let Some(Reverse(worst)) = heap.peek()
            && similarity > worst.similarity
        {
            heap.pop();
            heap.push(Reverse(Scored {
                similarity,
                ordinal,
                embedding,
            }));
        }
    }

    // `into_sorted_vec` is ascending in `Reverse`, i.e. best first.
    Ok(heap
        .into_sorted_vec()
        .into_iter()
        .map(|Reverse(s)| SearchMatch {
            vector_id: s.ordinal,
            similarity: s.similarity,
            metadata: s.embedding.to_metadata(),
        })
        .collect())
}

impl RealRocksDBStorage {
    /// Reject vectors this storage was not configured for.
    fn validate_vector(&self, vector: &[f32]) -> Result<()> {
        match self.vector_dimension {
            Some(dim) if vector.len() != dim => Err(anyhow::anyhow!(
                "Invalid embedding dimension: expected {}, got {}",
                dim,
                vector.len()
            )),
            None if vector.is_empty() => {
                Err(anyhow::anyhow!("Invalid embedding dimension: empty vector"))
            }
            _ => Ok(()),
        }
    }

    /// Save an embedding to RocksDB
    pub async fn save_embedding(&self, embedding: &StoredEmbedding) -> Result<()> {
        self.batch_save_embeddings(std::slice::from_ref(embedding))
            .await
    }

    /// Load all embeddings for a session
    pub async fn load_session_embeddings(&self, session_id: &str) -> Result<Vec<StoredEmbedding>> {
        self.load_embeddings_with_prefix(format!("embedding:{}:", session_id))
            .await
    }

    /// Load all embeddings from storage (migration / export).
    pub async fn load_all_embeddings(&self) -> Result<Vec<StoredEmbedding>> {
        self.load_embeddings_with_prefix(EMBEDDING_PREFIX.to_string())
            .await
    }

    async fn load_embeddings_with_prefix(&self, prefix: String) -> Result<Vec<StoredEmbedding>> {
        let db = self.db.clone();
        blocking(move || {
            let mut embeddings = Vec::new();
            for item in scan_prefix(&db, prefix.as_bytes()) {
                let (_, value) = item?;
                if let Some(embedding) = decode_embedding(&value) {
                    embeddings.push(embedding);
                }
            }
            Ok(embeddings)
        })
        .await
    }

    /// Delete an embedding
    pub async fn delete_embedding(&self, session_id: &str, content_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = embedding_key(session_id, content_id);
        let index_key = content_index_key(content_id);

        blocking(move || {
            let existed = db.get(key.as_bytes())?.is_some();
            let mut batch = WriteBatch::default();
            batch.delete(key.as_bytes());
            // Only drop the index entry if it points at this session.
            if db.get(index_key.as_bytes())?.as_deref() == Some(session_of_key(&key)) {
                batch.delete(index_key.as_bytes());
            }
            db.write(batch)?;
            Ok(existed)
        })
        .await
    }

    /// Count embeddings for a session (keys only, no decoding).
    pub async fn count_embeddings(&self, session_id: &str) -> usize {
        let db = self.db.clone();
        let prefix = format!("embedding:{}:", session_id);
        blocking(move || Ok(keys_with_prefix(&db, prefix.as_bytes())?.len()))
            .await
            .unwrap_or(0)
    }

    /// Batch save embeddings in a single RocksDB WriteBatch.
    pub async fn batch_save_embeddings(&self, embeddings: &[StoredEmbedding]) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }

        let mut batch = WriteBatch::default();
        for embedding in embeddings {
            put_embedding(&mut batch, embedding)?;
        }
        let count = embeddings.len();
        let db = self.db.clone();
        blocking(move || {
            db.write(batch)?;
            debug!("Batch saved {} embeddings", count);
            Ok(())
        })
        .await
    }
}

/// Session id part of an `embedding:{session_id}:{content_id}` key.
fn session_of_key(key: &str) -> &[u8] {
    let rest = &key[EMBEDDING_PREFIX.len()..];
    let end = rest.find(':').unwrap_or(rest.len());
    &rest.as_bytes()[..end]
}

#[async_trait]
impl VectorStorage for RealRocksDBStorage {
    async fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String> {
        self.validate_vector(&vector)?;
        let id = metadata.id.clone();
        self.save_embedding(&StoredEmbedding::new(vector, metadata))
            .await?;
        Ok(id)
    }

    async fn add_vectors_batch(
        &self,
        vectors: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> Result<Vec<String>> {
        for (vector, _) in &vectors {
            self.validate_vector(vector)?;
        }
        let ids = vectors.iter().map(|(_, m)| m.id.clone()).collect();
        let embeddings: Vec<StoredEmbedding> = vectors
            .into_iter()
            .map(|(vector, metadata)| StoredEmbedding::new(vector, metadata))
            .collect();
        self.batch_save_embeddings(&embeddings).await?;
        Ok(ids)
    }

    /// Exact cosine search over all persisted embeddings.
    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchMatch>> {
        let db = self.db.clone();
        let query = query.to_vec();
        blocking(move || scan_search(&db, EMBEDDING_PREFIX, &query, k, |_| true)).await
    }

    /// Exact cosine search over one session's embeddings (prefix scan).
    async fn search_in_session(
        &self,
        query: &[f32],
        k: usize,
        session_id: &str,
    ) -> Result<Vec<SearchMatch>> {
        let db = self.db.clone();
        let query = query.to_vec();
        let prefix = format!("embedding:{}:", session_id);
        blocking(move || scan_search(&db, &prefix, &query, k, |_| true)).await
    }

    /// Exact cosine search restricted to one content type.
    async fn search_by_content_type(
        &self,
        query: &[f32],
        k: usize,
        content_type: &str,
    ) -> Result<Vec<SearchMatch>> {
        let db = self.db.clone();
        let query = query.to_vec();
        let content_type = content_type.to_string();
        blocking(move || {
            scan_search(&db, EMBEDDING_PREFIX, &query, k, |e| {
                e.content_type == content_type
            })
        })
        .await
    }

    /// Remove the embedding stored for content id `id`.
    async fn remove_vector(&self, id: &str) -> Result<bool> {
        let db = self.db.clone();
        let content_id = id.to_string();
        blocking(move || delete_by_content_id(&db, &content_id)).await
    }

    async fn has_session_embeddings(&self, session_id: &str) -> bool {
        let db = self.db.clone();
        let prefix = format!("embedding:{}:", session_id);
        blocking(move || Ok(scan_prefix(&db, prefix.as_bytes()).next().is_some()))
            .await
            .unwrap_or(false)
    }

    async fn count_session_embeddings(&self, session_id: &str) -> usize {
        self.count_embeddings(session_id).await
    }

    async fn total_count(&self) -> usize {
        let db = self.db.clone();
        blocking(move || Ok(keys_with_prefix(&db, EMBEDDING_PREFIX.as_bytes())?.len()))
            .await
            .unwrap_or(0)
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
