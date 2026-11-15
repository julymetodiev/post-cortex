// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The embedding engine port (ARCHITECTURE section 5): the model, known
//! without loading it, and one call that embeds a batch. Adapters load
//! their model on first use.

use async_trait::async_trait;
use post_cortex_core::archive::EmbeddingModelInfo;

use crate::error::Result;

/// Turns texts into vectors.
#[async_trait]
pub trait EmbeddingEngine: Send + Sync {
    /// The model's id and the length of its vectors, known without loading
    /// it.
    fn model(&self) -> EmbeddingModelInfo;

    /// One vector per text, in input order (none for no text). The first
    /// call loads the model.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
}
