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

//! Hash-based static embedding fallback. Used by \[`EmbeddingModelType::StaticSimilarityMRL`\].
//!
//! **Warning**: this is a synthetic embedding — every text is mapped to a
//! deterministic, pseudo-random unit vector keyed by its hash. Semantically
//! similar inputs receive completely unrelated vectors. Useful only for tests
//! and for keeping the engine functional when BERT loading is unavailable.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::embeddings::backend::EmbeddingBackend;
use crate::embeddings::pool::MemoryPool;

/// Fallback embedding backend — hashes the input bytes into a
/// deterministic pseudo-embedding of the configured dimension.
///
/// Used when the BERT backend is unavailable (e.g. the `bert` feature
/// is disabled or the HuggingFace Hub model cache cannot be reached).
/// The output is **not** semantically meaningful — it just provides
/// stable vectors for tests and offline development.
pub struct StaticHashBackend {
    dimension: usize,
    memory_pool: Arc<MemoryPool>,
}

impl StaticHashBackend {
    pub(in crate::embeddings) fn new(dimension: usize, memory_pool: Arc<MemoryPool>) -> Self {
        Self {
            dimension,
            memory_pool,
        }
    }

    fn text_hash(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

#[async_trait]
impl EmbeddingBackend for StaticHashBackend {
    fn embedding_dimension(&self) -> usize {
        self.dimension
    }

    fn is_bert_based(&self) -> bool {
        false
    }

    async fn process_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());

        for text in &texts {
            let mut embedding = self.memory_pool.get_or_allocate();
            let hash = Self::text_hash(text);
            embedding.clear();
            embedding.reserve(self.dimension);

            for i in 0..self.dimension {
                let value = ((hash.wrapping_add(i as u64)) as f32 / u64::MAX as f32) * 2.0 - 1.0;
                embedding.push(value);
            }

            // Normalize to unit length
            let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in &mut embedding {
                    *val /= norm;
                }
            }

            results.push(embedding);
        }

        Ok(results)
    }
}
