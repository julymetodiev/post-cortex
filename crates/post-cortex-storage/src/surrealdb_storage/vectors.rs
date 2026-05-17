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

//! [`VectorStorage`] trait implementation for [`SurrealDBStorage`].
//!
//! Uses SurrealDB's native HNSW index — see the `idx_embedding_hnsw` index in
//! `core::initialize_schema` — for O(log n) similarity search.

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use surrealdb::types::SurrealValue;
use tracing::debug;

use crate::traits::VectorStorage;
use post_cortex_embeddings::{SearchMatch, VectorMetadata};

use super::MIN_VECTOR_LEN;
use super::SurrealDBStorage;
use super::records::{EmbeddingRecord, KnnResult};

#[async_trait]
impl VectorStorage for SurrealDBStorage {
    async fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String> {
        if vector.len() < MIN_VECTOR_LEN {
            return Err(anyhow::anyhow!(
                "Vector too short: got {} dims, need at least {}",
                vector.len(),
                MIN_VECTOR_LEN,
            ));
        }

        debug!(
            "SurrealDBStorage: Adding vector for content {}",
            metadata.id
        );

        let record = EmbeddingRecord {
            content_id: metadata.id.clone(),
            session_id: metadata.source.clone(),
            vector,
            text: metadata.text,
            content_type: metadata.content_type,
            timestamp: metadata.timestamp.to_rfc3339(),
            metadata: metadata.metadata,
        };

        let _: Option<EmbeddingRecord> = self
            .db
            .upsert(("embedding", metadata.id.clone()))
            .content(record)
            .await?;

        Ok(metadata.id)
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

    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchMatch>> {
        if query.len() < MIN_VECTOR_LEN {
            return Err(anyhow::anyhow!(
                "Query vector dimension mismatch: at least {}, got {}",
                MIN_VECTOR_LEN,
                query.len()
            ));
        }

        debug!("SurrealDBStorage: HNSW search for {} nearest vectors", k);

        // Use native HNSW KNN search - O(log n) instead of O(n)!
        // The <|K|> operator uses the HNSW index for fast approximate nearest neighbor search
        let query_vec: Vec<f32> = query.to_vec();
        let mut response = self
            .db
            .query("SELECT *, vector::distance::knn() AS distance FROM embedding WHERE vector <|$k|> $query_vec")
            .bind(("k", k as i64))
            .bind(("query_vec", query_vec))
            .await?;

        let records: Vec<KnnResult> = response.take(0)?;

        let matches: Vec<SearchMatch> = records
            .into_iter()
            .map(|r| {
                // Convert distance to similarity (cosine distance -> similarity)
                let similarity = 1.0 - r.distance.min(1.0);
                SearchMatch {
                    vector_id: 0,
                    similarity,
                    metadata: VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                }
            })
            .collect();

        debug!("SurrealDBStorage: HNSW found {} matches", matches.len());

        Ok(matches)
    }

    async fn search_in_session(
        &self,
        query: &[f32],
        k: usize,
        session_id: &str,
    ) -> Result<Vec<SearchMatch>> {
        if query.len() < MIN_VECTOR_LEN {
            return Err(anyhow::anyhow!(
                "Query vector dimension mismatch: at least {}, got {}",
                MIN_VECTOR_LEN,
                query.len()
            ));
        }

        debug!(
            "SurrealDBStorage: HNSW search for {} nearest vectors in session {}",
            k, session_id
        );

        // Use HNSW with post-filtering: fetch more results, filter by session
        // This is more efficient than full table scan for large datasets
        let fetch_count = (k * 5).max(50); // Fetch 5x more to ensure enough after filtering
        let query_vec: Vec<f32> = query.to_vec();

        let mut response = self
            .db
            .query("SELECT *, vector::distance::knn() AS distance FROM embedding WHERE vector <|$fetch_count|> $query_vec")
            .bind(("fetch_count", fetch_count as i64))
            .bind(("query_vec", query_vec))
            .await?;

        let records: Vec<KnnResult> = response.take(0)?;

        // Post-filter by session and convert to SearchMatch
        let matches: Vec<SearchMatch> = records
            .into_iter()
            .filter(|r| r.session_id == session_id)
            .take(k)
            .map(|r| {
                let similarity = 1.0 - r.distance.min(1.0);
                SearchMatch {
                    vector_id: 0,
                    similarity,
                    metadata: VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                }
            })
            .collect();

        debug!(
            "SurrealDBStorage: HNSW found {} matches in session",
            matches.len()
        );

        Ok(matches)
    }

    async fn search_by_content_type(
        &self,
        query: &[f32],
        k: usize,
        content_type: &str,
    ) -> Result<Vec<SearchMatch>> {
        if query.len() < MIN_VECTOR_LEN {
            return Err(anyhow::anyhow!(
                "Query vector dimension mismatch: at least {}, got {}",
                MIN_VECTOR_LEN,
                query.len()
            ));
        }

        debug!(
            "SurrealDBStorage: HNSW search for {} nearest vectors of type {}",
            k, content_type
        );

        // Use HNSW with post-filtering by content type
        let fetch_count = (k * 5).max(50);
        let query_vec: Vec<f32> = query.to_vec();

        let mut response = self
            .db
            .query("SELECT *, vector::distance::knn() AS distance FROM embedding WHERE vector <|$fetch_count|> $query_vec")
            .bind(("fetch_count", fetch_count as i64))
            .bind(("query_vec", query_vec))
            .await?;

        let records: Vec<KnnResult> = response.take(0)?;

        // Post-filter by content_type
        let matches: Vec<SearchMatch> = records
            .into_iter()
            .filter(|r| r.content_type == content_type)
            .take(k)
            .map(|r| {
                let similarity = 1.0 - r.distance.min(1.0);
                SearchMatch {
                    vector_id: 0,
                    similarity,
                    metadata: VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                }
            })
            .collect();

        debug!(
            "SurrealDBStorage: HNSW found {} matches of type {}",
            matches.len(),
            content_type
        );

        Ok(matches)
    }

    async fn remove_vector(&self, id: &str) -> Result<bool> {
        let result: Option<EmbeddingRecord> = self.delete("embedding", id).await?;
        Ok(result.is_some())
    }

    async fn has_session_embeddings(&self, session_id: &str) -> bool {
        let count = self.count_session_embeddings(session_id).await;
        count > 0
    }

    async fn count_session_embeddings(&self, session_id: &str) -> usize {
        let result = self
            .db
            .query("SELECT count() FROM embedding WHERE session_id = $session_id GROUP ALL")
            .bind(("session_id", session_id.to_string()))
            .await;

        if let Ok(mut response) = result {
            #[derive(Deserialize, SurrealValue)]
            struct CountResult {
                count: i64,
            }
            if let Ok(Some(count)) = response.take::<Option<CountResult>>(0) {
                return count.count as usize;
            }
        }

        0
    }

    async fn total_count(&self) -> usize {
        let result = self
            .db
            .query("SELECT count() FROM embedding GROUP ALL")
            .await;

        if let Ok(mut response) = result {
            #[derive(Deserialize, SurrealValue)]
            struct CountResult {
                count: i64,
            }
            if let Ok(Some(count)) = response.take::<Option<CountResult>>(0) {
                return count.count as usize;
            }
        }

        0
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let mut response = self
            .db
            .query("SELECT * FROM embedding WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        let records: Vec<EmbeddingRecord> = response.take(0)?;

        Ok(records
            .into_iter()
            .map(|r| {
                (
                    r.vector,
                    VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                )
            })
            .collect())
    }

    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let mut all_vectors = Vec::new();
        let limit = 1000;
        let mut start = 0;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM embedding LIMIT $limit START $start")
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EmbeddingRecord> = response.take(0)?;

            if records.is_empty() {
                break;
            }

            for r in records {
                all_vectors.push((
                    r.vector,
                    VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                ));
            }

            start += limit;
        }

        Ok(all_vectors)
    }
}
