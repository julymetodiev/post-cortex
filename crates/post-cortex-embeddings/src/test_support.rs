// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Test doubles for crates that need an embedding engine without a model
//! download. Compiled for this crate's tests and with the `test-support`
//! feature (enable it from a dev-dependency only).

use crate::embeddings::{EmbeddingBackend, EmbeddingConfig, LocalEmbeddingEngine};
use crate::error::Result;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Maps each text to a deterministic pseudo-random unit vector: equal texts
/// get equal vectors, different texts nearly orthogonal ones. Not semantic,
/// so a test that needs ranking by meaning must use a real model.
pub struct HashBackend {
    dimension: usize,
}

impl HashBackend {
    /// A backend producing `dimension`-dim vectors.
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let seed = hasher.finish();
        // SplitMix64 stream seeded by the text hash: component i is the
        // i-th output, mapped to [-1, 1).
        let mut vector: Vec<f32> = (0..self.dimension as u64)
            .map(|i| {
                let x = splitmix64(seed.wrapping_add(i.wrapping_mul(0x9E37_79B9_7F4A_7C15)));
                ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
            })
            .collect();
        let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            vector.iter_mut().for_each(|x| *x /= norm);
        }
        vector
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

impl EmbeddingBackend for HashBackend {
    fn embedding_dimension(&self) -> usize {
        self.dimension
    }

    fn batch_size(&self) -> usize {
        1024
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.vector(t)).collect())
    }
}

/// An engine over a [`HashBackend`] of `dimension`.
pub fn hash_engine(dimension: usize) -> LocalEmbeddingEngine {
    LocalEmbeddingEngine::with_backend(
        Arc::new(HashBackend::new(dimension)),
        EmbeddingConfig::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector_db::cosine_similarity;

    #[tokio::test]
    async fn different_texts_get_nearly_orthogonal_unit_vectors() {
        let engine = hash_engine(256);
        let a = engine
            .encode_text("store the ledger in RocksDB")
            .await
            .unwrap();
        let b = engine.encode_text("the cat sat on the mat").await.unwrap();
        let again = engine
            .encode_text("store the ledger in RocksDB")
            .await
            .unwrap();
        assert_eq!(a, again);
        assert_eq!(a.len(), 256);
        let norm = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "{norm}");
        let cos = cosine_similarity(&a, &b);
        assert!(
            cos.abs() < 0.5,
            "unrelated texts must not be similar: {cos}"
        );
    }
}
