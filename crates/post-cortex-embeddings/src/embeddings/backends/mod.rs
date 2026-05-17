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

//! Concrete [`EmbeddingBackend`](super::backend::EmbeddingBackend) implementations.
//!
//! - [`StaticHashBackend`] — hash-based fallback, always compiled in. Used
//!   only for `EmbeddingModelType::StaticSimilarityMRL` and as the last
//!   resort when both `bert` and `model2vec` features are disabled.
//! - [`BertBackend`] (feature `bert`) — Candle + HuggingFace Hub BERT
//!   transformer models (MiniLM / multilingual / TinyBERT / BGE).
//! - [`Model2VecBackend`] (feature `model2vec`, default) — `model2vec-rs`
//!   static embeddings for the Potion family (multilingual / code).

#[cfg(feature = "bert")]
pub mod bert;
#[cfg(feature = "model2vec")]
pub mod model2vec;
pub mod static_hash;

#[cfg(feature = "bert")]
pub use bert::BertBackend;
#[cfg(feature = "model2vec")]
pub use model2vec::Model2VecBackend;
pub use static_hash::StaticHashBackend;
