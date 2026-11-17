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

/// Configuration for the embedding engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model type for embeddings
    pub model_type: EmbeddingModelType,
    /// Batches running at once on the blocking pool (one semaphore for
    /// every backend).
    pub max_concurrent_ops: usize,
    /// Longest a batch may wait for a slot and run, in seconds.
    pub operation_timeout_secs: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_type: EmbeddingModelType::default(),
            max_concurrent_ops: num_cpus::get(),
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum EmbeddingModelType {
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
    /// Every model, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::MiniLM,
        Self::MultilingualMiniLM,
        Self::TinyBERT,
        Self::BGESmall,
        Self::PotionMultilingual,
        Self::PotionCode,
    ];

    /// The model's name in configuration (the variant name; also its serde
    /// name). [`FromStr`](std::str::FromStr) inverts it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MiniLM => "MiniLM",
            Self::MultilingualMiniLM => "MultilingualMiniLM",
            Self::TinyBERT => "TinyBERT",
            Self::BGESmall => "BGESmall",
            Self::PotionMultilingual => "PotionMultilingual",
            Self::PotionCode => "PotionCode",
        }
    }

    /// Get embedding dimension for this model type.
    ///
    /// The Potion dimensions are taken from each model's published
    /// config — `Model2VecBackend` verifies the runtime dimension at load
    /// time and panics if they disagree, so this constant stays the source
    /// of truth for HNSW index sizing.
    pub fn embedding_dimension(&self) -> usize {
        match self {
            Self::MiniLM | Self::MultilingualMiniLM | Self::BGESmall => 384,
            Self::TinyBERT => 312,
            // potion-multilingual-128M outputs 256-dim vectors.
            Self::PotionMultilingual => 256,
            // potion-code-16M outputs 256-dim vectors (measured on the Hub's
            // snapshot 86848193; 512 was never true, so it never loaded).
            Self::PotionCode => 256,
        }
    }

    /// Get model ID for HuggingFace Hub.
    pub fn model_id(&self) -> &'static str {
        match self {
            Self::MiniLM => "sentence-transformers/all-MiniLM-L6-v2",
            Self::MultilingualMiniLM => {
                "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
            }
            Self::TinyBERT => "huawei-noah/TinyBERT_General_6L_312D",
            Self::BGESmall => "BAAI/bge-small-en-v1.5",
            Self::PotionMultilingual => "minishlab/potion-multilingual-128M",
            Self::PotionCode => "minishlab/potion-code-16M",
        }
    }

    /// Check if this is a Model2Vec static-embedding model.
    pub fn is_model2vec(&self) -> bool {
        matches!(self, Self::PotionMultilingual | Self::PotionCode)
    }
}

impl std::fmt::Display for EmbeddingModelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A name that is no embedding model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown embedding model {0:?} (expected one of MiniLM, MultilingualMiniLM, TinyBERT, BGESmall, PotionMultilingual, PotionCode)"
)]
pub struct UnknownModel(pub String);

/// Strict inverse of [`EmbeddingModelType::as_str`]: no fallback.
impl std::str::FromStr for EmbeddingModelType {
    type Err = UnknownModel;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str() == s)
            .ok_or_else(|| UnknownModel(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_round_trip_and_unknown_names_fail() {
        for model in EmbeddingModelType::ALL {
            assert_eq!(model.as_str().parse::<EmbeddingModelType>(), Ok(model));
            assert_eq!(
                serde_json::to_value(model).unwrap(),
                serde_json::Value::String(model.as_str().into())
            );
        }
        assert!("potion".parse::<EmbeddingModelType>().is_err());
        assert!("".parse::<EmbeddingModelType>().is_err());
    }
}
