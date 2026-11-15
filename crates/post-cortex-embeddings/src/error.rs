// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The errors of `post-cortex-embeddings`: every public operation returns
//! [`Result<T>`], with the kind a caller acts on.

use thiserror::Error;

/// Errors produced by [`crate`].
#[derive(Debug, Error)]
pub enum Error {
    /// The model could not be downloaded or opened.
    #[error("loading model {model}: {reason}")]
    ModelLoad {
        /// The model id.
        model: String,
        /// What failed.
        reason: String,
    },
    /// The model needs a feature this build was compiled without.
    #[error("model {model} needs the `{feature}` feature, which this build lacks")]
    FeatureDisabled {
        /// The model.
        model: String,
        /// The missing feature.
        feature: &'static str,
    },
    /// The tokenizer could not be read or could not encode the input.
    #[error("tokenization failed: {0}")]
    Tokenization(String),
    /// The model's forward pass failed.
    #[error("inference failed: {0}")]
    Inference(String),
    /// The model produced no vector, or an empty one.
    #[error("model {0} produced no embedding")]
    NoEmbedding(String),
    /// A vector of another length than the index's or the model's.
    #[error("vector dimension mismatch: expected {expected}, got {actual}")]
    Dimension {
        /// The dimension expected.
        expected: usize,
        /// The vector's length.
        actual: usize,
    },
    /// The index holds its configured maximum.
    #[error("vector index is full: {max} vectors")]
    IndexFull {
        /// The configured maximum.
        max: usize,
    },
    /// The engine was shut down.
    #[error("embedding engine is not active")]
    Inactive,
    /// The caller stopped waiting, so the rest of the batch was not run.
    #[error("embedding batch abandoned by its caller")]
    Abandoned,
    /// A batch took longer than the configured timeout.
    #[error("embedding batch timed out after {0} s")]
    Timeout(u64),
    /// A blocking model task panicked or was cancelled.
    #[error("blocking model task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[cfg(feature = "bert")]
impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Error::Inference(e.to_string())
    }
}

#[cfg(feature = "rerank")]
impl From<ort::Error> for Error {
    fn from(e: ort::Error) -> Self {
        Error::Inference(e.to_string())
    }
}

/// Crate-level result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for post_cortex_ports::PortError {
    fn from(e: Error) -> Self {
        post_cortex_ports::PortError::Unavailable(e.to_string())
    }
}
