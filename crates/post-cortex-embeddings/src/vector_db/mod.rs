// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The in-memory vector database: a contiguous arena of L2-normalised
//! rows with tag columns, exact scans (split over the rayon pool when
//! long), and word statistics with optional BM25 postings. [`VectorDB`] is
//! the entry point; the in-memory `SearchIndex` adapter is built on it.

mod arena;
mod catalog;
mod common;
mod database;
pub mod similarity;
mod text_index;
pub mod types;

#[cfg(test)]
mod tests;

pub use database::VectorDB;
pub use similarity::{cosine_similarity, dot, l2_norm, normalize_in_place};
pub use types::BulkInsert;
