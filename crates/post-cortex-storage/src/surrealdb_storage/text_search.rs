// Copyright (c) 2025, 2026 Julius ML
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

//! BM25 search over `embedding.text` through the `idx_embedding_text`
//! full-text index (see `core::initialize_schema`).
//!
//! The query is matched with `@1,OR@`: any query term may match, ranked by
//! BM25. Callers pass only words that are not too common (the vector DB's
//! `query_terms`, by document frequency): a word on most rows adds nothing to
//! the ranking, and the index scores its postings across the whole table
//! before the session filter applies, so a raw natural-language question can
//! take seconds on a large database.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;
use surrealdb::types::SurrealValue;
use tracing::debug;

use post_cortex_embeddings::text::tokens;
use post_cortex_embeddings::{Corpus, MESSAGE_CONTENT_TYPE, SearchMatch, VectorMetadata};

use super::SurrealDBStorage;

/// Upper bound for one full-text query; past it the caller falls back to
/// vector search alone.
const TEXT_SEARCH_TIMEOUT: &str = "500ms";

/// One full-text hit, with its cosine similarity to the query vector.
#[derive(Debug, Deserialize, SurrealValue)]
struct TextHit {
    content_id: String,
    session_id: String,
    text: String,
    content_type: String,
    timestamp: String,
    metadata: HashMap<String, String>,
    /// BM25 score; only the order is used.
    #[allow(dead_code)]
    score: f32,
    similarity: f32,
}

impl SurrealDBStorage {
    /// BM25 top-`k` for `query` over the texts in `corpus`, optionally
    /// restricted to `sources`.
    pub(super) async fn text_search(
        &self,
        query: &str,
        query_vector: &[f32],
        k: usize,
        sources: &[String],
        corpus: Corpus,
    ) -> Result<Vec<SearchMatch>> {
        let terms = tokens(query).join(" ");
        if terms.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let mut scope = String::new();
        if !sources.is_empty() {
            scope.push_str("AND session_id IN $sources ");
        }
        match corpus {
            Corpus::All => {}
            Corpus::Records => scope.push_str("AND content_type != $message "),
            Corpus::Messages => scope.push_str("AND content_type = $message "),
        }
        // Rows written under an older embedding model can have another
        // dimension; cosine over mismatched lengths is an error.
        let sql = format!(
            "SELECT content_id, session_id, text, content_type, timestamp, metadata, \
                 search::score(1) AS score, \
                 IF array::len(vector) = array::len($qv) \
                     THEN vector::similarity::cosine(vector, $qv) ELSE 0 END AS similarity \
             FROM embedding WHERE text @1,OR@ $terms {scope}\
             ORDER BY score DESC LIMIT $k TIMEOUT {TEXT_SEARCH_TIMEOUT}"
        );
        let mut response = self
            .db
            .query(sql)
            .bind(("terms", terms))
            .bind(("qv", query_vector.to_vec()))
            .bind(("sources", sources.to_vec()))
            .bind(("message", MESSAGE_CONTENT_TYPE))
            .bind(("k", k as i64))
            .await?
            .check()?;
        let hits: Vec<TextHit> = response.take(0)?;
        debug!("SurrealDBStorage: BM25 found {} matches", hits.len());

        Ok(hits
            .into_iter()
            .map(|h| SearchMatch {
                vector_id: 0,
                similarity: h.similarity,
                metadata: VectorMetadata {
                    id: h.content_id,
                    text: h.text,
                    source: h.session_id,
                    content_type: h.content_type,
                    timestamp: Self::parse_datetime(&h.timestamp),
                    metadata: h.metadata,
                },
            })
            .collect())
    }
}
