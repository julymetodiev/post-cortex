// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Related content, the retrieval graph's records and candidates, and
//! vector counts.

use crate::content_vectorizer::ContentVectorizer;
use crate::content_vectorizer::types::{
    CHUNK_PARENT_KEY, ContentKind, HitSource, SearchOptions, SemanticSearchResult,
    VectorizationStats,
};
use post_cortex_core::graph::retrieval_graph::{self, GraphRecord};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::error::Result;
use post_cortex_ports::{EntrySelect, IndexFilter, IndexHit};
use tracing::debug;
use uuid::Uuid;

impl ContentVectorizer {
    /// Find related content across sessions (excluding the supplied session).
    ///
    /// # Errors
    /// Returns an error if the semantic search or query processing fails.
    pub async fn find_related_content(
        &self,
        session: &ActiveSession,
        topic: &str,
        limit: usize,
    ) -> Result<Vec<SemanticSearchResult>> {
        let results = self
            .search(
                topic,
                limit * 2,
                crate::content_vectorizer::VectorScope::All,
                SearchOptions::default(),
            )
            .await?;

        let filtered: Vec<_> = results
            .iter()
            .filter(|r| r.source != HitSource::Session(session.id()))
            .take(limit)
            .cloned()
            .collect();

        debug!(
            "Found {} related content items across sessions for topic: '{}'",
            filtered.len(),
            topic
        );

        Ok(filtered)
    }

    /// The session's update vectors as retrieval-graph records: content id,
    /// the terms and entity names stored in the vector's metadata. Vectors
    /// stored before terms were get them extracted from their text.
    ///
    /// # Errors
    /// The index could not be read.
    pub async fn graph_records(&self, session_id: Uuid) -> Result<Vec<GraphRecord<String>>> {
        let records: Vec<String> = ContentKind::ALL
            .iter()
            .filter(|kind| kind.is_record())
            .map(|kind| kind.as_str().to_string())
            .collect();
        Ok(self
            .index()
            .entries(&session_id.to_string(), &EntrySelect::Kinds(records))
            .await?
            .into_iter()
            .map(|e| e.meta)
            // Updates, not chunks: the first chunk carries the whole update's
            // terms.
            .filter(|m| !m.metadata.contains_key(CHUNK_PARENT_KEY))
            .map(|m| {
                let terms = match m.metadata.get(retrieval_graph::TERMS_KEY) {
                    Some(joined) => retrieval_graph::split_keys(joined)
                        .map(String::from)
                        .collect(),
                    None => retrieval_graph::terms(&m.text),
                };
                let entities = m
                    .metadata
                    .get(retrieval_graph::ENTITIES_KEY)
                    .map(|joined| {
                        retrieval_graph::split_keys(joined)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                GraphRecord {
                    id: m.id,
                    terms,
                    entities,
                }
            })
            .collect())
    }

    /// Index hits for the content ids the retrieval graph ranked, in its
    /// order, limited to `session` and `filter`.
    ///
    /// # Errors
    /// The index could not score them.
    pub async fn graph_matches(
        &self,
        ids: &[String],
        query_embedding: &[f32],
        session: &str,
        filter: &IndexFilter,
    ) -> Result<Vec<IndexHit>> {
        Ok(self
            .index()
            .score(query_embedding, session, ids)
            .await?
            .into_iter()
            .filter(|hit| filter.admits(&hit.meta))
            .collect())
    }

    /// Get statistics about vectorized content (sizes, dimension).
    ///
    /// # Errors
    /// The index could not be read.
    pub async fn get_vectorization_stats(&self) -> Result<VectorizationStats> {
        let index = self.index().stats().await?;
        Ok(VectorizationStats {
            total_vectors: index.entries,
            memory_usage_mb: index.memory_bytes / 1024 / 1024,
            embedding_dimension: self.embedding_engine().model().dimension,
        })
    }

    /// Count embeddings for a specific session.
    ///
    /// # Errors
    /// The index could not be read.
    pub async fn count_session_embeddings(&self, session_id: Uuid) -> Result<usize> {
        Ok(self
            .index()
            .stats()
            .await?
            .per_session
            .get(&session_id.to_string())
            .copied()
            .unwrap_or(0))
    }
}
