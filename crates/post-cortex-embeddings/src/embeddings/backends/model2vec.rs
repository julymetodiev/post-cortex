// Copyright (c) 2025 Julius ML
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

//! Model2Vec backend — static, multilingual embeddings via `model2vec-rs`.
//!
//! Loads `minishlab/potion-*` checkpoints from the HuggingFace Hub (or a
//! local path), encodes batches of text through the precomputed token-vector
//! table, and returns L2-normalised vectors. All computation is CPU-only and
//! orders of magnitude faster than the BERT path — typically ms/text on a
//! laptop CPU with no GPU.

use anyhow::Result;
use async_trait::async_trait;
use model2vec_rs::model::StaticModel;
use std::sync::Arc;
use tracing::{debug, info};

use crate::embeddings::backend::EmbeddingBackend;
use crate::embeddings::config::EmbeddingModelType;

/// Default token-length budget passed to `StaticModel::encode_with_args`.
/// Mirrors `model2vec-rs`' built-in default but kept explicit so callers
/// can audit the trade-off.
const DEFAULT_MAX_TOKENS: usize = 512;

/// Default batch size for the underlying `model2vec-rs` encode loop. The
/// surrounding `LocalEmbeddingEngine` already chunks input batches via its
/// own adaptive batching, so this just controls model2vec's inner loop.
const DEFAULT_INNER_BATCH: usize = 1024;

/// Static-embedding backend powered by `model2vec-rs`.
///
/// Construct via [`Self::load`] — the call is async and offloads the
/// (blocking) `StaticModel::from_pretrained` download/load onto
/// [`tokio::task::spawn_blocking`].
pub struct Model2VecBackend {
    model: Arc<StaticModel>,
    dimension: usize,
    model_type: EmbeddingModelType,
}

impl Model2VecBackend {
    /// Download (if needed) and load a Model2Vec model from the HuggingFace
    /// Hub. Verifies the runtime embedding dimension against the
    /// `EmbeddingModelType` constant and returns an error on mismatch —
    /// silent drift would corrupt every downstream HNSW index.
    pub async fn load(model_type: EmbeddingModelType) -> Result<Self> {
        let model_id = model_type.model_id().to_string();
        info!("Loading Model2Vec backend for model: {}", model_id);

        // `StaticModel::from_pretrained` does blocking I/O (HF Hub fetch
        // + safetensors mmap) — keep it off the reactor.
        let model = tokio::task::spawn_blocking({
            let model_id = model_id.clone();
            move || -> Result<StaticModel> {
                StaticModel::from_pretrained(model_id, None, None, None)
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Model2Vec load join error: {e}"))??;

        // Probe the actual dimension by encoding a sentinel — this is the
        // ground-truth source. If it disagrees with the enum constant the
        // HNSW index would be sized wrong, so fail loudly.
        let probe = model.encode_with_args(
            &["dimension probe".to_string()],
            Some(DEFAULT_MAX_TOKENS),
            DEFAULT_INNER_BATCH,
        );
        let runtime_dim = probe.first().map(|v| v.len()).unwrap_or(0);
        let declared_dim = model_type.embedding_dimension();
        if runtime_dim == 0 {
            return Err(anyhow::anyhow!(
                "Model2Vec ({model_id}) produced an empty vector on the dimension probe"
            ));
        }
        if runtime_dim != declared_dim {
            return Err(anyhow::anyhow!(
                "Model2Vec ({model_id}) reports dimension {runtime_dim} at runtime but \
                 EmbeddingModelType::{model_type:?}.embedding_dimension() = {declared_dim}; \
                 update the enum constant before shipping or HNSW indices will be sized wrong"
            ));
        }

        info!(
            "Model2Vec model loaded — id: {}, dim: {}",
            model_id, runtime_dim
        );

        Ok(Self {
            model: Arc::new(model),
            dimension: runtime_dim,
            model_type,
        })
    }
}

#[async_trait]
impl EmbeddingBackend for Model2VecBackend {
    fn embedding_dimension(&self) -> usize {
        self.dimension
    }

    /// Model2Vec is **not** BERT — the engine's BERT branch (concurrency
    /// controller, adaptive batching, timeout) is bypassed. The static
    /// path dispatches directly because each call is ms-cheap.
    fn is_bert_based(&self) -> bool {
        false
    }

    async fn process_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model = self.model.clone();
        let dim = self.dimension;
        let model_type = self.model_type;

        let embeddings = tokio::task::spawn_blocking(move || {
            model.encode_with_args(&texts, Some(DEFAULT_MAX_TOKENS), DEFAULT_INNER_BATCH)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Model2Vec encode join error: {e}"))?;

        // Sanity check — `encode_with_args` should produce one vector per
        // input. A mismatch means the underlying API changed under us.
        for (i, v) in embeddings.iter().enumerate() {
            if v.len() != dim {
                return Err(anyhow::anyhow!(
                    "Model2Vec ({model_type:?}) produced vector at index {i} with dim {} \
                     (expected {})",
                    v.len(),
                    dim
                ));
            }
        }

        debug!(
            "Model2Vec encoded {} texts (dim={}) for {:?}",
            embeddings.len(),
            dim,
            model_type
        );

        Ok(embeddings)
    }
}
