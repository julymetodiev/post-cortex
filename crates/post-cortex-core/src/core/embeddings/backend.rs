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

//! Pluggable embedding backend trait.
//!
//! [`LocalEmbeddingEngine`](super::engine::LocalEmbeddingEngine) wraps any
//! `EmbeddingBackend` and adds adaptive batching, concurrency control, timeouts,
//! and performance tracking around the backend's raw `process_batch` call.
//!
//! Drop-in replacements (e.g. `model2vec-rs` static embeddings) implement this
//! trait and become available to all callers without touching the engine.

use anyhow::Result;
use async_trait::async_trait;

/// A backend capable of producing embedding vectors for a batch of texts.
#[async_trait]
pub trait EmbeddingBackend: Send + Sync {
    /// Dimension of the produced embedding vectors.
    fn embedding_dimension(&self) -> usize;

    /// Whether this backend runs a BERT-style model. BERT backends are routed
    /// through the engine's concurrency controller, timeout, and adaptive
    /// batching machinery; non-BERT backends are invoked directly.
    fn is_bert_based(&self) -> bool;

    /// Process a batch of texts and return one embedding vector per text.
    /// The engine slices large inputs into chunks no larger than the engine's
    /// `current_batch_size` before calling this method.
    async fn process_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
}
