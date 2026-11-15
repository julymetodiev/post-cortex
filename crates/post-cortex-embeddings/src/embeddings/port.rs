// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The embedding engine port of `post-cortex-ports`, implemented by a
//! loaded [`LocalEmbeddingEngine`] and by [`LazyEmbeddingEngine`], which
//! loads its model on the first batch (T1: nothing loads at start).

use async_trait::async_trait;
use post_cortex_core::archive::EmbeddingModelInfo;
use post_cortex_ports::{EmbeddingEngine, PortError};
use tokio::sync::OnceCell;

use crate::embeddings::{EmbeddingConfig, LocalEmbeddingEngine};

#[async_trait]
impl EmbeddingEngine for LocalEmbeddingEngine {
    fn model(&self) -> EmbeddingModelInfo {
        EmbeddingModelInfo {
            model: self.model_type().as_str().to_string(),
            dimension: self.embedding_dimension(),
        }
    }

    async fn embed(&self, texts: Vec<String>) -> post_cortex_ports::error::Result<Vec<Vec<f32>>> {
        self.encode_batch(texts)
            .await
            .map_err(|e| PortError::Unavailable(format!("{e:#}")))
    }
}

/// The engine of an [`EmbeddingConfig`], loaded on the first
/// [`EmbeddingEngine::embed`] (a failed load is retried by the next call).
pub struct LazyEmbeddingEngine {
    config: EmbeddingConfig,
    engine: OnceCell<LocalEmbeddingEngine>,
}

impl LazyEmbeddingEngine {
    /// The engine of `config`, not loaded yet.
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            engine: OnceCell::new(),
        }
    }

    async fn engine(&self) -> post_cortex_ports::error::Result<&LocalEmbeddingEngine> {
        self.engine
            .get_or_try_init(|| async {
                LocalEmbeddingEngine::new(self.config.clone())
                    .await
                    .map_err(|e| {
                        PortError::Unavailable(format!(
                            "Failed to initialize embedding engine: {e:#}"
                        ))
                    })
            })
            .await
    }
}

#[async_trait]
impl EmbeddingEngine for LazyEmbeddingEngine {
    fn model(&self) -> EmbeddingModelInfo {
        EmbeddingModelInfo {
            model: self.config.model_type.as_str().to_string(),
            dimension: self.config.model_type.embedding_dimension(),
        }
    }

    async fn embed(&self, texts: Vec<String>) -> post_cortex_ports::error::Result<Vec<Vec<f32>>> {
        self.engine().await?.embed(texts).await
    }
}
