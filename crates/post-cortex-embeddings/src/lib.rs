// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding engines, rerankers and the in-memory vector index of
//! post-cortex.
//!
//! This crate is self-contained: anyone needing a Model2Vec or
//! Candle-backed BERT embedder or an exact vector index can depend on it
//! without pulling the post-cortex daemon, storage, or orchestrator.
//! Implements [`EmbeddingBackend`] (Model2Vec, BERT via Candle) and ships
//! [`VectorDB`] (a normalised vector arena with exact scans).
//!
//! The types the vector ports speak (`VectorMetadata` and friends) and the
//! [`Reranker`] port live in `post-cortex-ports`; this crate implements
//! them.

// SAFETY: candle's `from_mmaped_safetensors` needs an unsafe block at the
// single call site (file mmap). The exemption is justified inline.
#![deny(unsafe_code)]

mod blocking;
pub mod embeddings;
pub mod error;
pub mod index;
pub mod rerank;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod vector_db;

pub use error::{Error, Result};
pub use index::MemoryIndex;

pub use post_cortex_ports::Reranker;

pub use embeddings::{
    EmbeddingBackend, EmbeddingConfig, EmbeddingModelType, LazyEmbeddingEngine,
    LocalEmbeddingEngine,
};
pub use vector_db::{BulkInsert, VectorDB, cosine_similarity, dot, l2_norm, normalize_in_place};
