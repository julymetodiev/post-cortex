// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The types the vector ports speak: vector metadata, the content kind of
//! stored message vectors and corpora.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Content type of raw conversation messages (the message stream), as
/// opposed to structured records (context updates, entity descriptions).
pub const MESSAGE_CONTENT_TYPE: &str = "Message";

/// Metadata key of a record or message vector: who wrote it, when known.
pub const AUTHOR_KEY: &str = "author";

/// Which stored texts a search covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Corpus {
    /// Structured records only (the default).
    #[default]
    Records,
    /// Raw messages only.
    Messages,
    /// Both.
    All,
}

impl Corpus {
    /// Whether a vector of `content_type` belongs to this corpus.
    pub fn admits(self, content_type: &str) -> bool {
        match self {
            Self::All => true,
            Self::Records => content_type != MESSAGE_CONTENT_TYPE,
            Self::Messages => content_type == MESSAGE_CONTENT_TYPE,
        }
    }

    /// Parse the lowercase name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "records" => Some(Self::Records),
            "messages" => Some(Self::Messages),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

/// Metadata for a vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMetadata {
    /// Unique identifier for the vector
    pub id: String,
    /// Original text content, shared by the index, its hits and the
    /// reranker without copies.
    pub text: Arc<str>,
    /// Source identifier (e.g., session_id, update_id)
    pub source: String,
    /// Content type classification
    pub content_type: String,
    /// Timestamp when vector was added
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Additional custom metadata
    pub metadata: HashMap<String, String>,
}

impl VectorMetadata {
    /// Create new metadata with required fields
    pub fn new(
        id: String,
        text: impl Into<Arc<str>>,
        source: String,
        content_type: String,
    ) -> Self {
        Self {
            id,
            text: text.into(),
            source,
            content_type,
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Date the vector by its source content, not by when it was embedded:
    /// a backfill (`vectorize_session`) otherwise makes old records look new
    /// to recency-biased search.
    #[must_use]
    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Add custom metadata
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;

    #[test]
    fn with_timestamp_keeps_the_source_time() {
        let then = chrono::DateTime::parse_from_rfc3339("2026-03-24T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let m = VectorMetadata::new("u1".into(), "t", "s".into(), "UpdateContent".into())
            .with_timestamp(then);
        assert_eq!(m.timestamp, then);
    }
}
