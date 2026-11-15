// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The reranker port: scores each candidate document against the query so
//! the candidate pool from vector and full-text search can be reordered.

use crate::error::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Scores (query, document) pairs.
#[async_trait]
pub trait Reranker: Send + Sync {
    /// One relevance in [0, 1] per document, in input order; higher is more
    /// relevant. The documents are shared with the index's entries.
    async fn score(&self, query: &str, documents: &[Arc<str>]) -> Result<Vec<f32>>;
}
