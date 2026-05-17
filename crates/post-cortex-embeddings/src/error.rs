// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Typed error hierarchy for `post-cortex-embeddings`.
//!
//! Public API on this crate returns [`Result<T>`] (aliased to
//! `Result<T, Error>`). Internal helpers may still use `anyhow::Result`
//! for ergonomic error context — the boundary functions wrap them with
//! [`Error::External`].
//!
//! Variants:
//! - [`Error::ModelLoad`] — failed to download or open the safetensors
//!   model file (hf-hub network / disk / format issues).
//! - [`Error::Tokenization`] — tokenizer JSON parse failure or
//!   encoding mismatch.
//! - [`Error::Inference`] — candle tensor op error during the forward
//!   pass.
//! - [`Error::Dimension`] — embedding dimension mismatch when an
//!   external vector is fed into the wrong index.
//! - [`Error::IndexFull`] — HNSW index reached its configured capacity.
//! - [`Error::External`] — last-resort wrapper around `anyhow::Error`
//!   for migrating call sites.

use thiserror::Error;

/// Errors produced by [`crate`].
#[derive(Debug, Error)]
pub enum Error {
    /// Failed to load the embedding model file (network / disk / format).
    #[error("embedding model load failed: {0}")]
    ModelLoad(String),

    /// Tokeniser failed to parse its config or encode the input string.
    #[error("tokenization failed: {0}")]
    Tokenization(String),

    /// Forward pass returned a candle / tensor error.
    #[error("inference failed: {0}")]
    Inference(String),

    /// Caller supplied a vector with a dimension other than the
    /// index's expected width.
    #[error("vector dimension mismatch: expected {expected}, got {actual}")]
    Dimension { expected: usize, actual: usize },

    /// HNSW index is full.
    #[error("vector index is full")]
    IndexFull,

    /// Catch-all for migrating call sites that still use `anyhow`.
    #[error(transparent)]
    External(#[from] anyhow::Error),
}

/// Crate-level result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// A stable discriminant for metrics / structured logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ModelLoad(_) => "model_load",
            Self::Tokenization(_) => "tokenization",
            Self::Inference(_) => "inference",
            Self::Dimension { .. } => "dimension_mismatch",
            Self::IndexFull => "index_full",
            Self::External(_) => "external",
        }
    }
}
