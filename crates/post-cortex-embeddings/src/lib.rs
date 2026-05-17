// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding engines + HNSW vector database for post-cortex.
//!
//! This crate is self-contained: anyone needing a Candle-backed BERT
//! embedder or an HNSW index for nearest-neighbour search can depend
//! on it without pulling the post-cortex daemon, storage, or
//! orchestrator. Implements [`EmbeddingBackend`] (BERT via Candle,
//! static-hash fallback) and ships [`VectorDB`] (HNSW with optional
//! product quantization).
//!
//! Shared types — [`post_cortex_embeddings::VectorMetadata`]
//! and friends — live in `post-cortex-core` so the storage trait API
//! can reference them without depending on the ML stack.

// SAFETY: candle's `from_mmaped_safetensors` needs an unsafe block at the
// single call site (file mmap). The exemption is justified inline.
#![deny(unsafe_code)]

pub mod embeddings;
pub mod error;
pub mod vector_db;

pub use error::{Error, Result};

pub use embeddings::{EmbeddingBackend, EmbeddingConfig, EmbeddingModelType, LocalEmbeddingEngine};
pub use vector_db::{
    ProductQuantizationCodebook, SearchMatch, SearchMode, SearchQualityPreset, StoredVector,
    VectorDB, VectorDbConfig, VectorDbStats, VectorDbStatsSnapshot, VectorMetadata,
};
