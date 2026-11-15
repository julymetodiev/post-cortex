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

//! The [`VectorStore`] port on SurrealDB: the `embedding` table, keyed
//! `{session}_{content id}`. Search (vectors and BM25) runs on the memory
//! layer's in-memory index loaded from this table.

use crate::port_error::PortResult;
use crate::port_error::{AdapterError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealdb::types::SurrealValue;

use post_cortex_ports::VectorMetadata;
use post_cortex_ports::storage::VectorStore;

use crate::surrealdb_storage::MIN_VECTOR_LEN;
use crate::surrealdb_storage::SurrealDBStorage;
use crate::surrealdb_storage::records::EmbeddingRecord;

/// The metadata of a stored embedding row; an unparsable timestamp is
/// [`Corrupt`](post_cortex_ports::PortError::Corrupt).
pub fn stored_metadata(
    id: String,
    text: String,
    source: String,
    content_type: String,
    timestamp: &str,
    metadata: std::collections::HashMap<String, String>,
) -> Result<VectorMetadata> {
    Ok(VectorMetadata {
        timestamp: post_cortex_ports::error::parse_timestamp("embedding", &id, timestamp)?,
        id,
        text: text.into(),
        source,
        content_type,
        metadata,
    })
}

/// The row of a `SELECT count() ... GROUP ALL`.
#[derive(Deserialize, SurrealValue)]
struct CountRow {
    count: i64,
}

/// The count of a `GROUP ALL` row; no row means no matches.
fn count_of(row: Option<CountRow>) -> Result<usize> {
    row.map_or(Ok(0), |c| Ok(usize::try_from(c.count)?))
}

/// A stored embedding row as a vector and its metadata.
fn stored_vector(r: EmbeddingRecord) -> Result<(Vec<f32>, VectorMetadata)> {
    let metadata = stored_metadata(
        r.content_id,
        r.text,
        r.session_id,
        r.content_type,
        &r.timestamp,
        r.metadata,
    )?;
    Ok((r.vector, metadata))
}

pub fn embedding_record(vector: Vec<f32>, metadata: VectorMetadata) -> EmbeddingRecord {
    EmbeddingRecord {
        content_id: metadata.id,
        session_id: metadata.source,
        vector,
        text: metadata.text.to_string(),
        content_type: metadata.content_type,
        timestamp: metadata.timestamp.to_rfc3339(),
        metadata: metadata.metadata,
    }
}

/// Embedding row for the batched upsert.
#[derive(Serialize, SurrealValue)]
struct KeyedEmbedding {
    /// `{session_id}_{content_id}`: content ids repeat across sessions.
    key: String,
    record: EmbeddingRecord,
}

/// Upsert embeddings under session-scoped record ids, in one query.
///
/// Records written before ids were session-scoped are keyed by the bare
/// content id; the same session's legacy record is dropped first (the
/// `(session_id, content_id)` index is unique), other sessions' records
/// are left alone.
async fn upsert_embeddings(db: &Surreal<Any>, records: Vec<EmbeddingRecord>) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let rows: Vec<KeyedEmbedding> = records
        .into_iter()
        .map(|record| KeyedEmbedding {
            key: format!("{}_{}", record.session_id, record.content_id),
            record,
        })
        .collect();
    db.query(
        "FOR $e IN $rows { \
             DELETE type::record('embedding', $e.record.content_id) \
                 WHERE session_id = $e.record.session_id; \
             UPSERT type::record('embedding', $e.key) CONTENT $e.record; };",
    )
    .bind(("rows", rows))
    .await?
    .check()?;
    Ok(())
}

impl SurrealDBStorage {
    /// Reject vectors this storage was not configured for.
    fn validate_vector(&self, vector: &[f32]) -> Result<()> {
        match self.vector_dimension {
            Some(dim) if vector.len() != dim => Err(AdapterError::Dimension {
                expected: dim,
                got: vector.len(),
            }),
            None if vector.len() < MIN_VECTOR_LEN => Err(AdapterError::TooShort {
                got: vector.len(),
                min: MIN_VECTOR_LEN,
            }),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl VectorStore for SurrealDBStorage {
    /// All vectors in one query.
    async fn upsert_vectors(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> PortResult<()> {
        self.breaker
            .guard(async {
                for (vector, _) in &vectors {
                    self.validate_vector(vector)?;
                }
                let records = vectors
                    .into_iter()
                    .map(|(vector, metadata)| embedding_record(vector, metadata))
                    .collect();
                upsert_embeddings(&self.db(), records).await
            })
            .await
    }

    /// One query, under current or legacy record ids.
    async fn delete_vectors(&self, session_id: &str, content_ids: &[String]) -> PortResult<usize> {
        self.breaker
            .guard(async {
                if content_ids.is_empty() {
                    return Ok(0);
                }
                let mut response = self
            .db()
            .query(
                "DELETE embedding WHERE session_id = $session_id AND content_id IN $content_ids \
                 RETURN BEFORE",
            )
            .bind(("session_id", session_id.to_string()))
            .bind(("content_ids", content_ids.to_vec()))
            .await?
            .check()?;
                let removed: Vec<EmbeddingRecord> = response.take(0)?;
                Ok(removed.len())
            })
            .await
    }

    async fn has_session_embeddings(&self, session_id: &str) -> PortResult<bool> {
        self.breaker
            .guard(async { Ok(self.count_session_embeddings(session_id).await? > 0) })
            .await
    }

    async fn count_session_embeddings(&self, session_id: &str) -> PortResult<usize> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query("SELECT count() FROM embedding WHERE session_id = $session_id GROUP ALL")
                    .bind(("session_id", session_id.to_string()))
                    .await?
                    .check()?;
                count_of(response.take(0)?)
            })
            .await
    }

    async fn total_count(&self) -> PortResult<usize> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query("SELECT count() FROM embedding GROUP ALL")
                    .await?
                    .check()?;
                count_of(response.take(0)?)
            })
            .await
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> PortResult<Vec<(Vec<f32>, VectorMetadata)>> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query("SELECT * FROM embedding WHERE session_id = $session_id")
                    .bind(("session_id", session_id.to_string()))
                    .await?
                    .check()?;
                let records: Vec<EmbeddingRecord> = response.take(0)?;
                records.into_iter().map(stored_vector).collect()
            })
            .await
    }

    async fn get_all_vectors(&self) -> PortResult<Vec<(Vec<f32>, VectorMetadata)>> {
        self.breaker
            .guard(async {
                let mut all_vectors = Vec::new();
                let limit = 1000;
                let mut start = 0;
                loop {
                    let mut response = self
                        .db()
                        .query("SELECT * FROM embedding LIMIT $limit START $start")
                        .bind(("limit", limit))
                        .bind(("start", start))
                        .await?
                        .check()?;
                    let records: Vec<EmbeddingRecord> = response.take(0)?;
                    if records.is_empty() {
                        break;
                    }
                    for r in records {
                        all_vectors.push(stored_vector(r)?);
                    }
                    start += limit;
                }
                Ok(all_vectors)
            })
            .await
    }
}
