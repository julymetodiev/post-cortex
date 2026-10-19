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

//! Public types: content classification, semantic search results, configuration, search options.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::query_cache::QueryCacheConfig;
use post_cortex_embeddings::EmbeddingConfig;
use post_cortex_embeddings::VectorDbConfig;

/// Content types for vectorization
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContentType {
    /// Update content from context updates
    UpdateContent,
    /// Entity descriptions
    EntityDescription,
    /// User questions and messages
    UserMessage,
    /// Decision points and rationale
    DecisionPoint,
    /// Code snippets and technical content
    CodeSnippet,
    /// Problem-solution pairs
    ProblemSolution,
    /// General session metadata
    SessionMetadata,
}

impl ContentType {
    /// Get the importance weight for this content type
    #[must_use]
    pub const fn importance_weight(&self) -> f32 {
        match self {
            Self::DecisionPoint => 1.0,
            Self::ProblemSolution => 0.9,
            Self::UserMessage => 0.8,
            Self::UpdateContent => 0.7,
            Self::CodeSnippet => 0.6,
            Self::EntityDescription => 0.5,
            Self::SessionMetadata => 0.3,
        }
    }
}

/// Search result with enhanced metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchResult {
    /// Unique identifier of the vectorised content.
    pub content_id: String,
    /// Session that owns this content.
    pub session_id: Uuid,
    /// Classification of the content (decision, code, etc.).
    pub content_type: ContentType,
    /// Raw text that was vectorised.
    pub text_content: String,
    /// Cosine similarity to the query vector.
    pub similarity_score: f32,
    /// Importance weight derived from [`ContentType::importance_weight`].
    pub importance_score: f32,
    /// When the content was originally stored.
    pub timestamp: DateTime<Utc>,
    /// Blended score combining similarity and importance.
    pub combined_score: f32,
    /// Set when this record was not matched itself but returned as a
    /// neighbour (an adjacent update in the same session) of the hit with
    /// this content id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neighbour_of: Option<String>,
}

impl SemanticSearchResult {
    /// Interpret the similarity score with a human-readable quality level
    #[must_use]
    pub const fn similarity_quality(&self) -> &'static str {
        if self.similarity_score >= 0.85 {
            "Excellent"
        } else if self.similarity_score >= 0.70 {
            "Very Good"
        } else if self.similarity_score >= 0.55 {
            "Good"
        } else if self.similarity_score >= 0.40 {
            "Moderate"
        } else if self.similarity_score >= 0.30 {
            "Fair"
        } else {
            "Weak"
        }
    }

    /// Get a detailed explanation of how the combined score was calculated
    #[must_use]
    pub fn score_explanation(&self) -> String {
        format!(
            "Combined Score: {:.4} = (Similarity: {:.4} × 0.7) + (Importance: {:.2} × 0.3) | Quality: {}",
            self.combined_score,
            self.similarity_score,
            self.importance_score,
            self.similarity_quality()
        )
    }

    /// Check if this result meets a given quality threshold
    #[must_use]
    pub fn meets_quality(&self, threshold: &str) -> bool {
        let min_score = match threshold {
            "excellent" => 0.85,
            "very_good" => 0.70,
            "good" => 0.55,
            "moderate" => 0.40,
            "fair" => 0.30,
            _ => 0.0,
        };
        self.similarity_score >= min_score
    }
}

/// Metadata key naming the update a chunk vector belongs to; set on every
/// chunk but the first (whose content id is the update's id).
pub const CHUNK_PARENT_KEY: &str = "parent";

/// Configuration for content vectorization
#[derive(Debug, Clone)]
pub struct ContentVectorizerConfig {
    /// Embedding model configuration.
    pub embedding_config: EmbeddingConfig,
    /// Vector database configuration.
    pub vector_db_config: VectorDbConfig,
    /// Minimum character length for text to be vectorised.
    pub min_text_length: usize,
    /// Updates longer than this many characters are embedded as several
    /// overlapping chunks of about this size (the rerankers see about 1000
    /// characters, so a longer passage would be cut off for them).
    pub max_text_length: usize,
    /// Number of items per vectorisation batch.
    pub batch_size: usize,
    /// Whether to vectorise entity descriptions.
    pub enable_entity_vectorization: bool,
    /// Whether to allow search across all sessions.
    pub enable_cross_session_search: bool,
    /// Configuration for the query result cache.
    pub query_cache_config: QueryCacheConfig,
    /// Whether to enable query result caching.
    pub enable_query_caching: bool,
    /// Temporal decay factor for recency bias in search results
    /// 0.0 = disabled (default, backward compatible)
    /// 0.1-0.5 = soft bias toward recent content
    /// 1.0+ = aggressive bias toward recent content
    pub recency_bias: f32,
}

impl Default for ContentVectorizerConfig {
    fn default() -> Self {
        Self {
            embedding_config: EmbeddingConfig::default(),
            vector_db_config: VectorDbConfig::default(),
            min_text_length: 10,
            max_text_length: 1000,
            batch_size: 32,
            enable_entity_vectorization: true,
            enable_cross_session_search: true,
            query_cache_config: QueryCacheConfig::default(),
            enable_query_caching: true,
            recency_bias: 0.0, // Disabled by default for backward compatibility
        }
    }
}

/// Search options for semantic queries
///
/// This struct consolidates all optional parameters for semantic search,
/// eliminating the need for multiple method variants (e.g., _with_recency).
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Maximum number of results to return (None = use default)
    pub limit: Option<usize>,

    /// Optional date range filter (start, end)
    pub date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,

    /// Recency bias parameter (0.0 = disabled, higher = more recent content preferred)
    pub recency_bias: Option<f32>,

    /// Ranking effort (None = the configured default)
    pub mode: Option<SearchMode>,

    /// Content ids ranked by the session's retrieval graph, best first
    /// (`post_cortex_core::graph::retrieval_graph`). In a single-session
    /// search that is reranked they join the vector and BM25 candidates.
    pub graph_candidates: Option<std::sync::Arc<[String]>>,
}

/// How much work a search puts into ranking. LoCoMo all@50 with ±1
/// neighbours: `fast` 0.789, `local` 0.870 (MiniLM-L4), `accurate` 0.938
/// (Voyage rerank-3-lite).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Vector and BM25 rankings fused, no reranker (~40 ms).
    Fast,
    /// The fused top 300 reranked by the local cross-encoder (~1 s on CPU).
    Local,
    /// Reranked by the remote reranker (Voyage), the local one as fallback;
    /// the local one alone when no remote reranker is configured.
    Accurate,
}

impl std::str::FromStr for SearchMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "local" => Ok(Self::Local),
            "accurate" => Ok(Self::Accurate),
            other => Err(format!(
                "unknown search mode '{other}' (expected fast, local or accurate)"
            )),
        }
    }
}
