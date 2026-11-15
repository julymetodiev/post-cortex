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
//! A backend is plain synchronous CPU work. [`LocalEmbeddingEngine`](crate::embeddings::engine::LocalEmbeddingEngine)
//! alone decides where and when it runs: on the blocking pool, behind one
//! semaphore, within a deadline, in batches of [`EmbeddingBackend::batch_size`].

use crate::error::Result;

/// A backend capable of producing embedding vectors for a batch of texts.
pub trait EmbeddingBackend: Send + Sync {
    /// Dimension of the produced embedding vectors.
    fn embedding_dimension(&self) -> usize;

    /// Texts per [`Self::embed`] call: the engine slices longer inputs.
    fn batch_size(&self) -> usize;

    /// One embedding vector per text, in input order. Blocks the calling
    /// thread for the whole batch.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}
