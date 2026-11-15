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

pub use post_cortex_core::core::content::ContentKind;

/// Size of the search index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VectorizationStats {
    /// Vectors in the index.
    pub total_vectors: usize,
    /// Memory the index uses, in MiB.
    pub memory_usage_mb: usize,
    /// Dimension of the embeddings.
    pub embedding_dimension: usize,
}

/// What a document hit is: its kind and the version found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentHit {
    /// The document's kind, `<module>/<name>`.
    pub kind: String,
    /// Its latest version.
    pub version: u64,
}

/// Where a search hit lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HitSource {
    /// A session's record, entity or turn.
    Session(Uuid),
    /// A workspace's document.
    Workspace(Uuid),
}

impl HitSource {
    /// The session, for a session's hit.
    #[must_use]
    pub const fn session(self) -> Option<Uuid> {
        match self {
            Self::Session(id) => Some(id),
            Self::Workspace(_) => None,
        }
    }

    /// The workspace, for a document hit.
    #[must_use]
    pub const fn workspace(self) -> Option<Uuid> {
        match self {
            Self::Workspace(id) => Some(id),
            Self::Session(_) => None,
        }
    }

    /// The session's or the workspace's id: the source of its index entry.
    #[must_use]
    pub const fn id(self) -> Uuid {
        match self {
            Self::Session(id) | Self::Workspace(id) => id,
        }
    }
}

/// Search result with enhanced metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchResult {
    /// Unique identifier of the vectorised content.
    pub content_id: String,
    /// Where the content lives: a session (records, entities, turns) or a
    /// workspace (documents).
    pub source: HitSource,
    /// Classification of the content (decision, code, etc.).
    pub content_type: ContentKind,
    /// Raw text that was vectorised.
    pub text_content: String,
    /// Cosine similarity to the query vector.
    pub similarity_score: f32,
    /// Importance weight derived from [`ContentKind::importance_weight`].
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
    /// Who wrote the record or message, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// For a record: its title, completed from its session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// For a record: the captured messages it was drawn from
    /// (`message:<seq>`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// For a record: when it held, if it no longer does (superseded,
    /// contradicted, restated, duplicate or forgotten; see
    /// `post_cortex_core::graph::temporal`). Such records rank lower when
    /// a later record or `forget` ended them explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validity: Option<post_cortex_core::graph::temporal::Validity>,
    /// For a record: how sure its writer was. Records below 1 rank lower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// Which rankings found the result and how it scored in each, when the
    /// search asked for a trace ([`SearchOptions::trace`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signals: Option<Signals>,
    /// For a document: its kind and version, completed from its latest
    /// version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<DocumentHit>,
    /// Set when `text_content` is the passage of a longer text the search
    /// showed for its query ([`SearchOptions::passage`]): the whole text's
    /// length, in characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_chars: Option<usize>,
}

/// How one result was found: its place in each
/// ranking (1 = first; none: not in that ranking's candidates), the fused
/// score and the reranker's.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Signals {
    /// Rank in the vector search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_rank: Option<usize>,
    /// Cosine similarity in the vector search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_similarity: Option<f32>,
    /// Rank in the BM25 search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bm25_rank: Option<usize>,
    /// Rank among the retrieval graph's candidates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_rank: Option<usize>,
    /// Reciprocal rank fusion score, when more than one ranking had
    /// candidates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fused: Option<f32>,
    /// Reranker score, when the search was reranked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank: Option<f32>,
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

/// Metadata key of a message vector: `user` or `assistant`.
pub const MESSAGE_ROLE_KEY: &str = "role";

pub use post_cortex_ports::AUTHOR_KEY;

/// Metadata key of a message vector: the turn's position in its stream.
pub const MESSAGE_SEQ_KEY: &str = "seq";

/// Metadata key of a document vector: the document's kind
/// (`<module>/<name>`).
pub const DOCUMENT_KIND_KEY: &str = "document_kind";

/// Configuration for content vectorization (the model and the index are
/// the adapters' configuration)
#[derive(Debug, Clone)]
pub struct ContentVectorizerConfig {
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

    /// Structured records, raw messages or both (default: records).
    pub corpus: post_cortex_ports::Corpus,

    /// Only records and messages by this author (default: anyone).
    pub author: Option<String>,

    /// Rank records and messages by this author a little higher
    /// ([`PREFER_AUTHOR_BOOST`]); others still compete.
    pub prefer_author: Option<String>,

    /// Record which rankings found each result ([`Signals`]). Traced
    /// searches skip the query cache.
    pub trace: bool,

    /// Only records of these interaction types (empty: every result).
    /// Applied by the Search capability to the ranked hits, by each
    /// record's own type: entity and message hits are left out when set.
    pub update_types: Vec<post_cortex_core::core::context_update::UpdateType>,

    /// Show each hit's text as its passage for the query, at most this many
    /// characters (`post_cortex_core::text::passage`; a shorter text whole);
    /// none: every text whole. Applied by the Search capability to the
    /// completed hits.
    pub passage: Option<usize>,

    /// Leave out the neighbours of the hits (the records and turns around
    /// each, `SystemConfig::neighbour_radius`; default: with them).
    pub without_neighbours: bool,

    /// The remote reranker may rank this search (it receives the query and
    /// the candidates' text). Set by the Search capability from the
    /// workspaces of the scope: what a caller sets is replaced.
    pub remote_rerank: bool,
}

/// What [`SearchOptions::prefer_author`] adds to a matching result's
/// combined score: enough to break near-ties, not to lift a weak match
/// over a strong one.
pub const PREFER_AUTHOR_BOOST: f32 = 0.05;

/// How much work a search puts into ranking. LoCoMo all@50 with ±1
/// neighbours: `fast` 0.803, `local` 0.879 (MiniLM-L4), `accurate` 0.937
/// (Voyage rerank-3-lite), as docs/DAEMON_MODE.md gives them.
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
