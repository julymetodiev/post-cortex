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

//! Embedding-engine configuration and supported model types.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the embedding engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model type for embeddings
    pub model_type: EmbeddingModelType,
    /// Maximum batch size for processing
    pub max_batch_size: usize,
    /// Enable adaptive batch sizing
    pub adaptive_batching: bool,
    /// Memory pool size for vector reuse
    pub memory_pool_size: usize,
    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,
    /// Enable performance monitoring
    pub enable_performance_monitoring: bool,
    /// Model cache directory
    pub cache_dir: PathBuf,
    /// Enable model caching
    pub enable_caching: bool,
    /// Operation timeout in seconds
    pub operation_timeout_secs: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_type: EmbeddingModelType::default(),
            max_batch_size: 32,
            adaptive_batching: true,
            memory_pool_size: 1000,
            max_concurrent_ops: num_cpus::get() * 2,
            enable_performance_monitoring: true,
            cache_dir: PathBuf::from("./models_cache"),
            enable_caching: true,
            operation_timeout_secs: 30,
        }
    }
}

/// Embedding model types.
///
/// `PotionMultilingual` is the default — Model2Vec static embeddings, small
/// on disk, ms-per-text inference, multilingual (Latin / Cyrillic / CJK / …).
/// The BERT variants are kept for users who need transformer-grade semantic
/// quality and are willing to pay the GPU/CPU cost.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum EmbeddingModelType {
    /// Hash-based pseudo-embeddings — kept for backwards compatibility and
    /// as a last-resort fallback. **Does not** produce semantically
    /// meaningful vectors; only use when all real backends are disabled.
    StaticSimilarityMRL,
    /// MiniLM model (balanced performance, English-only). Requires the
    /// `bert` feature.
    MiniLM,
    /// Multilingual MiniLM model — 50+ languages including Bulgarian.
    /// Requires the `bert` feature.
    MultilingualMiniLM,
    /// TinyBERT model (smallest BERT variant). Requires the `bert` feature.
    TinyBERT,
    /// BGE Small model (balanced BERT). Requires the `bert` feature.
    BGESmall,
    /// `minishlab/potion-multilingual-128M` — Model2Vec static embeddings,
    /// multilingual (Latin/Cyrillic/CJK/…). Default — small, fast, no GPU.
    /// Requires the `model2vec` feature (on by default).
    #[default]
    PotionMultilingual,
    /// `minishlab/potion-code-16M` — Model2Vec static embeddings tuned for
    /// source code (English-leaning). Requires the `model2vec` feature.
    PotionCode,
}

impl EmbeddingModelType {
    /// Get embedding dimension for this model type.
    ///
    /// The Potion dimensions are taken from each model's published
    /// config — `Model2VecBackend` verifies the runtime dimension at load
    /// time and panics if they disagree, so this constant stays the source
    /// of truth for HNSW index sizing.
    pub fn embedding_dimension(&self) -> usize {
        match self {
            Self::StaticSimilarityMRL => 1024,
            Self::MiniLM | Self::MultilingualMiniLM | Self::BGESmall => 384,
            Self::TinyBERT => 312,
            // potion-multilingual-128M outputs 256-dim vectors.
            Self::PotionMultilingual => 256,
            // potion-code-16M outputs 512-dim vectors.
            Self::PotionCode => 512,
        }
    }

    /// Get model ID for HuggingFace Hub.
    pub fn model_id(&self) -> &'static str {
        match self {
            Self::StaticSimilarityMRL | Self::MiniLM => "sentence-transformers/all-MiniLM-L6-v2",
            Self::MultilingualMiniLM => {
                "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
            }
            Self::TinyBERT => "huawei-noah/TinyBERT_General_6L_312D",
            Self::BGESmall => "BAAI/bge-small-en-v1.5",
            Self::PotionMultilingual => "minishlab/potion-multilingual-128M",
            Self::PotionCode => "minishlab/potion-code-16M",
        }
    }

    /// Check if this is a BERT-based model (Candle transformer).
    pub fn is_bert_based(&self) -> bool {
        matches!(
            self,
            Self::MiniLM | Self::MultilingualMiniLM | Self::TinyBERT | Self::BGESmall
        )
    }

    /// Check if this is a Model2Vec static-embedding model.
    pub fn is_model2vec(&self) -> bool {
        matches!(self, Self::PotionMultilingual | Self::PotionCode)
    }
}
