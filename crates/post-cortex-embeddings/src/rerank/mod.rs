// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Rerankers: score each candidate document against the query so the
//! candidate pool from vector + full-text search can be reordered.
//!
//! - [`OnnxCrossEncoder`] (feature `rerank`) — a BERT cross-encoder such as
//!   `cross-encoder/ms-marco-MiniLM-L4-v2`, int8 on ONNX Runtime, local CPU.
//! - [`VoyageReranker`] (feature `rerank-voyage`) — Voyage AI's rerank API;
//!   candidate text leaves the machine.
//! - [`FallbackReranker`] — one reranker backed by another.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

#[cfg(feature = "rerank")]
mod onnx;
#[cfg(feature = "rerank-voyage")]
mod voyage;

#[cfg(feature = "rerank")]
pub use onnx::OnnxCrossEncoder;
#[cfg(feature = "rerank-voyage")]
pub use voyage::VoyageReranker;

/// Scores (query, document) pairs.
#[async_trait]
pub trait Reranker: Send + Sync {
    /// One relevance in [0, 1] per document, in input order; higher is more
    /// relevant.
    async fn score(&self, query: &str, documents: &[String]) -> Result<Vec<f32>>;
}

/// `primary`, or `fallback` when `primary` fails (e.g. a remote reranker
/// that is down or rate limited, backed by a local one).
pub struct FallbackReranker {
    primary: Arc<dyn Reranker>,
    fallback: Arc<dyn Reranker>,
}

impl FallbackReranker {
    /// Rerank with `primary`, falling back to `fallback` on error.
    pub fn new(primary: Arc<dyn Reranker>, fallback: Arc<dyn Reranker>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl Reranker for FallbackReranker {
    async fn score(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        match self.primary.score(query, documents).await {
            Ok(scores) => Ok(scores),
            Err(e) => {
                warn!("Reranker failed, using the fallback: {:#}", e);
                self.fallback.score(query, documents).await
            }
        }
    }
}
