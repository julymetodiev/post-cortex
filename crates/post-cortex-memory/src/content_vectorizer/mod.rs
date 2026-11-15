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

//! Content Vectorization Module
//!
//! Converts text content into vector representations for semantic search and
//! similarity analysis. Split into:
//! - [`types`] — public types and configuration
//! - [`vectorizer`] — the `ContentVectorizer` orchestrator and its configuration
//! - `store` — removing vectors and loading them back from storage
//! - `ingestion` — vectorization of sessions, updates, and entities
//! - `message_ingestion` — vectorization of captured messages
//! - `document_ingestion` — vectorization of modules' documents
//! - `vector_text` — the text, chunks and metadata a record or message becomes
//! - `search` — semantic search over one or several sessions
//! - `fusion` — rank fusion, reranking and traces
//! - `results` — hits as scored results
//! - `lookups` — related content, similarity, graph records, counts
//! - `cache` — query-cache management and recency-bias metrics

pub mod cache;
mod document_ingestion;
mod fusion;
pub mod ingestion;
mod lookups;
mod message_ingestion;
mod results;
pub mod search;
mod store;
pub mod types;
pub(crate) mod vector_text;
pub mod vectorizer;

pub(crate) use fusion::RERANK_POOL;
pub use search::VectorScope;
pub use types::{
    ContentKind, ContentVectorizerConfig, DocumentHit, HitSource, PREFER_AUTHOR_BOOST, SearchMode,
    SearchOptions, SemanticSearchResult, Signals, VectorizationStats,
};
pub use vectorizer::ContentVectorizer;

#[cfg(test)]
mod message_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod test_doubles;
#[cfg(test)]
mod tests;
