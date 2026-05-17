// Copyright (c) 2025, 2026 Julius ML
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

//! Vector Database with HNSW for semantic similarity search.
//!
//! HNSW-based approximate nearest neighbor search optimized for high-concurrency
//! workloads. Splits across submodules by concept: configuration, public types,
//! the Product Quantization codebook, the internal HNSW index, and the orchestrating
//! [`VectorDB`] type.

mod common;
pub mod config;
pub mod core;
mod hnsw_index;
pub mod product_quantization;
pub mod types;

#[cfg(test)]
mod tests;

pub use config::{SearchMode, SearchQualityPreset, VectorDbConfig};
pub use core::VectorDB;
pub use product_quantization::ProductQuantizationCodebook;
pub use types::{
    SearchMatch, StoredVector, VectorDbStats, VectorDbStatsSnapshot, VectorMetadata,
};
