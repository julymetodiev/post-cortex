// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`LazyCrossEncoder`]: the [`OnnxCrossEncoder`] of a model id, loaded on
//! the first score (T1: nothing loads at start; a failed load is retried
//! by the next call).

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_ports::{PortError, Reranker};
use tokio::sync::OnceCell;

use crate::rerank::OnnxCrossEncoder;

/// A cross-encoder loaded on first use.
pub struct LazyCrossEncoder {
    model: String,
    max_length: usize,
    runtime: Option<String>,
    loaded: OnceCell<OnnxCrossEncoder>,
}

impl LazyCrossEncoder {
    /// The cross-encoder `model` (see [`OnnxCrossEncoder::load`]), not
    /// loaded yet.
    pub fn new(model: String, max_length: usize, runtime: Option<String>) -> Self {
        Self {
            model,
            max_length,
            runtime,
            loaded: OnceCell::new(),
        }
    }
}

#[async_trait]
impl Reranker for LazyCrossEncoder {
    async fn score(
        &self,
        query: &str,
        documents: &[Arc<str>],
    ) -> post_cortex_ports::error::Result<Vec<f32>> {
        let encoder = self
            .loaded
            .get_or_try_init(|| async {
                OnnxCrossEncoder::load(&self.model, self.max_length, self.runtime.as_deref())
                    .await
                    .map_err(|e| {
                        PortError::Unavailable(format!(
                            "Failed to load reranker {}: {e:#}",
                            self.model
                        ))
                    })
            })
            .await?;
        encoder.score(query, documents).await
    }
}
