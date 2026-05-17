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
// VectorDB config + HNSW types have many tuning knobs — the
// field-by-field default-then-mutate pattern in tests is intentional.
#![allow(clippy::field_reassign_with_default)]
// `vec!` in tests is fine for clarity vs an array literal — silenced.
#![allow(clippy::useless_vec)]
// HNSW core uses .iter().enumerate() to index parallel arrays; the
// loop variable looks "unused" to the lint but is structurally needed.
#![allow(clippy::needless_range_loop)]
// We pin MSRV 1.85; some clippy suggestions point at stabilisations in
// later releases (e.g. div_ceil since 1.73 / inline_const since 1.79).
#![allow(clippy::incompatible_msrv)]

pub mod embeddings;
pub mod error;
pub mod vector_db;

pub use error::{Error, Result};

pub use embeddings::{EmbeddingBackend, EmbeddingConfig, EmbeddingModelType, LocalEmbeddingEngine};
pub use vector_db::{
    ProductQuantizationCodebook, SearchMatch, SearchMode, SearchQualityPreset, StoredVector,
    VectorDB, VectorDbConfig, VectorDbStats, VectorDbStatsSnapshot, VectorMetadata,
};
