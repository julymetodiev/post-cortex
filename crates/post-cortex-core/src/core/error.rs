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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//! Typed error handling for the Post-Cortex system
//!
//! This module defines a comprehensive error type `SystemError` that replaces
//! the use of `anyhow::Result` throughout the codebase, enabling:
//! - Pattern matching on specific error types
//! - Programmatic error recovery
//! - Better error messages for API consumers
//! - Type-safe error propagation

use thiserror::Error;
use uuid::Uuid;

/// System-wide error type for Post-Cortex
#[derive(Error, Debug)]
pub enum SystemError {
    // Storage errors
    #[error("Database error: {0}")]
    Database(#[from] rocksdb::Error),

    #[error("Session {0} not found")]
    SessionNotFound(Uuid),

    #[error("Workspace {0} not found")]
    WorkspaceNotFound(Uuid),

    #[error("Checkpoint {0} not found")]
    CheckpointNotFound(Uuid),

    // Serialization
    #[error("Serialization failed: {0}")]
    Serialization(String),

    #[error("Deserialization failed: {0}")]
    Deserialization(String),

    // Vector DB
    #[error("Vector dimension mismatch: expected {expected}, got {actual}")]
    VectorDimensionMismatch { expected: usize, actual: usize },

    #[error("HNSW index not built")]
    IndexNotBuilt,

    #[error("Vector {0} not found")]
    VectorNotFound(u32),

    #[error("Product Quantization error: {0}")]
    ProductQuantization(String),

    // Context processing
    #[error("Entity extraction failed: {0}")]
    EntityExtractionFailed(String),

    #[error("Update processing timeout after {0}ms")]
    UpdateTimeout(u64),

    #[error("Entity graph update failed: {0}")]
    GraphUpdateFailed(String),

    // Embeddings
    #[cfg(feature = "embeddings")]
    #[error("Embedding model error: {0}")]
    EmbeddingModel(String),

    #[cfg(feature = "embeddings")]
    #[error("Vectorization failed: {0}")]
    VectorizationFailed(String),

    // System
    #[error("Storage actor channel closed")]
    StorageActorDown,

    #[error("Operation timeout after {0}s")]
    OperationTimeout(u64),

    #[error("Circuit breaker open: {0}")]
    CircuitBreakerOpen(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    // I/O
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    // Task
    #[error("Task join error: {0}")]
    TaskJoin(String),

    // Catch-all for backward compatibility
    #[error("Internal error: {0}")]
    Internal(String),
}

// Bincode conversions
impl From<bincode::error::EncodeError> for SystemError {
    fn from(err: bincode::error::EncodeError) -> Self {
        SystemError::Serialization(err.to_string())
    }
}

impl From<bincode::error::DecodeError> for SystemError {
    fn from(err: bincode::error::DecodeError) -> Self {
        SystemError::Deserialization(err.to_string())
    }
}

// Tokio join error
impl From<tokio::task::JoinError> for SystemError {
    fn from(err: tokio::task::JoinError) -> Self {
        SystemError::TaskJoin(err.to_string())
    }
}

// Anyhow conversion for gradual migration
impl From<anyhow::Error> for SystemError {
    fn from(err: anyhow::Error) -> Self {
        SystemError::Internal(err.to_string())
    }
}

/// Type alias for Results using SystemError
pub type Result<T> = std::result::Result<T, SystemError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_not_found_error() {
        let id = Uuid::new_v4();
        let err = SystemError::SessionNotFound(id);
        assert_eq!(err.to_string(), format!("Session {} not found", id));
    }

    #[test]
    fn test_dimension_mismatch_error() {
        let err = SystemError::VectorDimensionMismatch {
            expected: 384,
            actual: 512,
        };
        assert_eq!(
            err.to_string(),
            "Vector dimension mismatch: expected 384, got 512"
        );
    }

    #[test]
    fn test_anyhow_conversion() {
        let anyhow_err = anyhow::anyhow!("test error");
        let system_err: SystemError = anyhow_err.into();
        assert!(matches!(system_err, SystemError::Internal(_)));
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let system_err: SystemError = io_err.into();
        assert!(matches!(system_err, SystemError::Io(_)));
    }
}
