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

/// Embedding model types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub enum EmbeddingModelType {
    /// Static embeddings (fast, lightweight)
    StaticSimilarityMRL,
    /// MiniLM model (balanced performance, English-only)
    MiniLM,
    /// Multilingual MiniLM model (supports 50+ languages including Bulgarian)
    #[default]
    MultilingualMiniLM,
    /// TinyBERT model (smallest BERT variant)
    TinyBERT,
    /// BGE Small model (balanced BERT)
    BGESmall,
}


impl EmbeddingModelType {
    /// Get embedding dimension for this model type
    pub fn embedding_dimension(&self) -> usize {
        match self {
            Self::StaticSimilarityMRL => 1024,
            Self::MiniLM | Self::MultilingualMiniLM | Self::BGESmall => 384,
            Self::TinyBERT => 312,
        }
    }

    /// Get model ID for HuggingFace Hub
    pub fn model_id(&self) -> &'static str {
        match self {
            Self::StaticSimilarityMRL | Self::MiniLM => "sentence-transformers/all-MiniLM-L6-v2",
            Self::MultilingualMiniLM => {
                "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
            }
            Self::TinyBERT => "huawei-noah/TinyBERT_General_6L_312D",
            Self::BGESmall => "BAAI/bge-small-en-v1.5",
        }
    }

    /// Check if this is a BERT-based model
    pub fn is_bert_based(&self) -> bool {
        matches!(
            self,
            Self::MiniLM | Self::MultilingualMiniLM | Self::TinyBERT | Self::BGESmall
        )
    }
}
