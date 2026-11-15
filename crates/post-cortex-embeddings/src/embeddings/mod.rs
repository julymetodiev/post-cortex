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
//! The public [`LocalEmbeddingEngine`] runs any [`EmbeddingBackend`] on the
//! blocking pool with one concurrency limit and a timeout. Backends live under
//! [`backends`]: `Model2VecBackend` (feature `model2vec`, default) and
//! `BertBackend` (feature `bert`, Candle + HuggingFace Hub).

pub mod backend;
pub mod backends;
pub mod config;
pub mod engine;
mod port;

#[cfg(test)]
mod tests;

pub use backend::EmbeddingBackend;
pub use config::{EmbeddingConfig, EmbeddingModelType};
pub use engine::LocalEmbeddingEngine;
pub use port::LazyEmbeddingEngine;
