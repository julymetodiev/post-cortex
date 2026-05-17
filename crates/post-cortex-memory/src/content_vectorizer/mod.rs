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
//! - [`vectorizer`] — the `ContentVectorizer` orchestrator and persistent-storage lifecycle
//! - `ingestion` — vectorization of sessions, updates, and entities
//! - `search` — semantic search, scoring, and similarity utilities
//! - `cache` — query-cache management and recency-bias metrics

pub mod cache;
pub mod ingestion;
pub mod search;
pub mod types;
pub mod vectorizer;

pub use types::{ContentType, ContentVectorizerConfig, SearchOptions, SemanticSearchResult};
pub use vectorizer::ContentVectorizer;
