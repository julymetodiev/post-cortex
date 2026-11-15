// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The persisted embeddings (by session and content id) of a storage
//! adapter: the port a `SearchIndex` adapter persists through, not used by
//! the capabilities.

use crate::error::Result;
use crate::vectors::VectorMetadata;
use async_trait::async_trait;

/// Persisted embeddings, keyed by (session, content id): `metadata.source`
/// names the session and `metadata.id` the content (an update id, a chunk
/// `<id>#<n>`, `entity:<name>`, `message:<seq>`). Search runs on the
/// in-memory index built from these; this port only stores them.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Store `vectors` in one batch (or transaction). Each one replaces the
    /// vector stored under its session and content id, so storing the same
    /// content again leaves one copy. A vector the adapter cannot store
    /// (e.g. of another dimension) fails the whole call.
    async fn upsert_vectors(&self, vectors: Vec<(Vec<f32>, VectorMetadata)>) -> Result<()>;

    /// Delete the vectors of `session_id` with these content ids, in one
    /// batch; other sessions' vectors under the same content ids stay.
    /// Returns how many were stored.
    async fn delete_vectors(&self, session_id: &str, content_ids: &[String]) -> Result<usize>;

    /// Whether the session has any stored vector.
    async fn has_session_embeddings(&self, session_id: &str) -> Result<bool>;

    /// Number of stored vectors of the session.
    async fn count_session_embeddings(&self, session_id: &str) -> Result<usize>;

    /// Number of stored vectors.
    async fn total_count(&self) -> Result<usize>;

    /// Every stored vector of the session (to load it into memory).
    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>>;

    /// Every stored vector (to load them into memory on startup).
    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>>;
}
