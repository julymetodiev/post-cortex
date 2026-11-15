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

//! Concrete [`EmbeddingBackend`](crate::embeddings::backend::EmbeddingBackend) implementations.
//!
//! - [`BertBackend`] (feature `bert`) — Candle + HuggingFace Hub BERT
//!   transformer models (MiniLM / multilingual / TinyBERT / BGE).
//! - [`Model2VecBackend`] (feature `model2vec`, default) — `model2vec-rs`
//!   static embeddings for the Potion family (multilingual / code).

#[cfg(feature = "bert")]
pub mod bert;
#[cfg(feature = "model2vec")]
pub mod model2vec;

#[cfg(feature = "bert")]
pub use bert::BertBackend;
#[cfg(feature = "model2vec")]
pub use model2vec::Model2VecBackend;
