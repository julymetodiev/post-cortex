// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! HNSW vector database — types + impl.
//!
//! [`types`] is dependency-light (only `serde`, `chrono`, `uuid`,
//! `std::sync::atomic`). The HNSW index + product-quantization codebook
//! live in private modules and are reachable through the [`VectorDB`]
//! entry point. Storage backends that only need to round-trip
//! [`VectorMetadata`] and [`SearchMatch`] can depend on
//! `post-cortex-embeddings` with `default-features = false`, which
//! compiles only this `types` module.

pub mod types;

// HNSW impl modules. Behind the `bert` feature today via the rest of the
// pipeline; we keep them unconditional so non-BERT consumers can still
// build the HNSW path with a custom embedding backend.
mod common;
pub mod config;
pub mod core;
mod hnsw_index;
pub mod product_quantization;

#[cfg(test)]
mod tests;

pub use config::{SearchMode, SearchQualityPreset, VectorDbConfig};
pub use core::VectorDB;
pub use product_quantization::ProductQuantizationCodebook;
pub use types::{SearchMatch, StoredVector, VectorDbStats, VectorDbStatsSnapshot, VectorMetadata};
