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

//! Local Embeddings Engine for semantic understanding.
//!
//! The public [`LocalEmbeddingEngine`] wraps any [`EmbeddingBackend`] with
//! adaptive batching, concurrency control, and timeouts. Backends live under
//! [`backends`] — currently [`BertBackend`](backends::BertBackend) (Candle +
//! HuggingFace Hub) and [`StaticHashBackend`](backends::StaticHashBackend)
//! (hash-based fallback). Swapping in `model2vec-rs` for the
//! `minishlab/potion-multilingual-128M` migration becomes a new backend impl
//! plus one branch in [`engine::LocalEmbeddingEngine::new`].

pub mod backend;
pub mod backends;
mod concurrency;
pub mod config;
pub mod engine;
mod pool;

#[cfg(test)]
mod tests;

pub use backend::EmbeddingBackend;
pub use config::{EmbeddingConfig, EmbeddingModelType};
pub use engine::LocalEmbeddingEngine;
