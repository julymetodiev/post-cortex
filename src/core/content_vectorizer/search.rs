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

//! Semantic search, score post-processing, and lightweight similarity helpers.

use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::core::vector_db::SearchMatch;
use crate::session::active_session::ActiveSession;

use super::types::{ContentType, SearchOptions, SemanticSearchResult};
use super::vectorizer::ContentVectorizer;

impl ContentVectorizer {
    /// Perform semantic search across all vectorized content.
    ///
    /// # Errors
    /// Returns an error if the query cannot be embedded or the vector database search fails.
    pub async fn semantic_search(
        &self,
        query: &str,
        limit: usize,
        session_filter: Option<Uuid>,
        options: SearchOptions,
    ) -> Result<Vec<SemanticSearchResult>> {
        let recency_bias = options.recency_bias.unwrap_or(self.config.recency_bias);
        let date_range = options.date_range;

        debug!("Performing semantic search for: '{}'", query);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        query.hash(&mut hasher);
        limit.hash(&mut hasher);
        if let Some(ref session_id) = session_filter {
            session_id.hash(&mut hasher);
        }
        if let Some(ref range) = date_range {
            range.0.timestamp().hash(&mut hasher);
            range.1.timestamp().hash(&mut hasher);
        }
        // Include recency_bias in cache key to prevent collisions
        if recency_bias != 0.0 {
            recency_bias.to_bits().hash(&mut hasher);
        }
        let params_hash = hasher.finish();

        // Check query cache first
        let query_embedding = if let Some(ref cache) = self.query_cache {
            let query_embedding = self.embedding_engine.encode_text(query).await?;
            if let Some(cached_results) = cache.search(query, &query_embedding, params_hash) {
                debug!("Cache hit for semantic search query: '{}'", query);
                return Ok(cached_results);
            }
            query_embedding
        } else {
            self.embedding_engine.encode_text(query).await?
        };

        let search_results = match (session_filter, date_range) {
            (Some(session_id), Some((start, end))) => self.vector_db.search_with_filter(
                &query_embedding,
                limit,
                |metadata| {
                    metadata.source == session_id.to_string()
                        && metadata.timestamp >= start
                        && metadata.timestamp <= end
                },
            )?,
            (Some(session_id), None) => self.vector_db.search_in_source(
                &query_embedding,
                limit,
                &session_id.to_string(),
            )?,
            (None, Some((start, end))) if self.config.enable_cross_session_search => self
                .vector_db
                .search_with_filter(&query_embedding, limit, |metadata| {
                    metadata.timestamp >= start && metadata.timestamp <= end
                })?,
            (None, None) if self.config.enable_cross_session_search => {
                self.vector_db.search(&query_embedding, limit)?
            }
            _ => return Ok(Vec::new()),
        };

        let results = self.process_search_results(search_results, Some(recency_bias))?;

        if let Some(ref cache) = self.query_cache
            && let Err(e) = cache.cache_results(
                query.to_string(),
                query_embedding,
                results.clone(),
                params_hash,
                session_filter,
            )
        {
            warn!("Failed to cache search results: {}", e);
        }

        Ok(results)
    }

    /// Perform semantic search within a specific set of sessions.
    ///
    /// This optimized version performs a single vector database search across all
    /// allowed sessions, then applies optional scoring adjustments (recency bias, etc.)
    /// in post-processing — avoiding the O(n²) cost of searching each session separately.
    pub async fn semantic_search_multisession(
        &self,
        query: &str,
        limit: usize,
        allowed_sessions: &[Uuid],
        options: SearchOptions,
    ) -> Result<Vec<SemanticSearchResult>> {
        let recency_bias = options.recency_bias.unwrap_or(self.config.recency_bias);
        let date_range = options.date_range;

        debug!(
            "Performing multisession semantic search with recency_bias={:?} for: '{}'",
            recency_bias, query
        );

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        query.hash(&mut hasher);
        limit.hash(&mut hasher);
        for session_id in allowed_sessions {
            session_id.hash(&mut hasher);
        }
        if let Some(ref range) = date_range {
            range.0.timestamp().hash(&mut hasher);
            range.1.timestamp().hash(&mut hasher);
        }
        if recency_bias != 0.0 {
            recency_bias.to_bits().hash(&mut hasher);
        }
        let params_hash = hasher.finish();

        let query_embedding = if let Some(ref cache) = self.query_cache {
            let query_embedding = self.embedding_engine.encode_text(query).await?;
            if let Some(cached_results) = cache.search(query, &query_embedding, params_hash) {
                debug!("Cache hit for multisession semantic search query: '{}'", query);
                return Ok(cached_results);
            }
            query_embedding
        } else {
            self.embedding_engine.encode_text(query).await?
        };

        let valid_sessions: std::collections::HashSet<String> =
            allowed_sessions.iter().map(|id| id.to_string()).collect();

        let search_results = if let Some((start, end)) = date_range {
            self.vector_db
                .search_with_filter(&query_embedding, limit, |metadata| {
                    valid_sessions.contains(&metadata.source)
                        && metadata.timestamp >= start
                        && metadata.timestamp <= end
                })?
        } else {
            self.vector_db
                .search_with_filter(&query_embedding, limit, |metadata| {
                    valid_sessions.contains(&metadata.source)
                })?
        };

        let results = self.process_search_results(search_results, Some(recency_bias))?;

        if let Some(ref cache) = self.query_cache
            && let Err(e) = cache.cache_results(
                query.to_string(),
                query_embedding,
                results.clone(),
                params_hash,
                None, // No session_filter for multisession
            )
        {
            warn!("Failed to cache search results: {}", e);
        }

        Ok(results)
    }

    /// Process raw vector search results into semantic results.
    fn process_search_results(
        &self,
        search_results: Vec<SearchMatch>,
        recency_bias_override: Option<f32>,
    ) -> Result<Vec<SemanticSearchResult>> {
        let now = Utc::now();
        // Defense-in-depth: clamp recency_bias to prevent negative or extreme values
        let recency_bias = recency_bias_override
            .unwrap_or(self.config.recency_bias)
            .clamp(0.0, 10.0);

        let score_adjuster: Option<Box<dyn crate::core::scoring::ScoreAdjuster>> = if recency_bias > 0.0 {
            Some(Box::new(crate::core::scoring::TemporalDecayAdjuster::new(
                recency_bias,
                now,
            )))
        } else {
            None
        };

        let decay_start = std::time::Instant::now();
        let mut decay_count = 0;

        let mut results = Vec::new();
        for result in search_results {
            let content_type = parse_content_type(&result.metadata.content_type);
            let session_id = Uuid::parse_str(&result.metadata.source)
                .context("Invalid session ID in metadata")?;

            let importance_weight = content_type.importance_weight();
            let base_score = result.similarity.mul_add(0.7, importance_weight * 0.3);

            let combined_score = if let Some(ref adjuster) = score_adjuster {
                decay_count += 1;
                adjuster.adjust(base_score, &result.metadata)
            } else {
                base_score
            };

            results.push(SemanticSearchResult {
                content_id: result.metadata.id,
                session_id,
                content_type,
                text_content: result.metadata.text,
                similarity_score: result.similarity,
                importance_score: importance_weight,
                timestamp: result.metadata.timestamp,
                combined_score,
            });
        }

        if decay_count > 0 {
            let decay_duration_ns = decay_start.elapsed().as_nanos() as u64;
            self.recency_bias_total_duration_ns
                .fetch_add(decay_duration_ns, Ordering::Relaxed);
            self.recency_bias_total_results
                .fetch_add(decay_count, Ordering::Relaxed);
            self.recency_bias_calculation_count
                .fetch_add(1, Ordering::Relaxed);
        }

        // Deduplicate results by content_id (keeps highest score or newest timestamp)
        let original_count = results.len();
        let mut dedup_map: HashMap<String, SemanticSearchResult> =
            HashMap::with_capacity(results.len());

        for result in results {
            let content_id = result.content_id.clone();
            match dedup_map.entry(content_id) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get();
                    if result.combined_score > existing.combined_score
                        || (result.combined_score == existing.combined_score
                            && result.timestamp > existing.timestamp)
                    {
                        entry.insert(result);
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(result);
                }
            }
        }

        let mut results: Vec<SemanticSearchResult> = dedup_map.into_values().collect();
        debug!(
            "Deduplicated {} results to {} unique",
            original_count,
            results.len()
        );

        results.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!("Found {} semantic search results", results.len());
        Ok(results)
    }

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
            .semantic_search(topic, limit * 2, None, SearchOptions::default())
            .await?;

        let filtered: Vec<_> = results
            .into_iter()
            .filter(|r| r.session_id != session.id())
            .take(limit)
            .collect();

        debug!(
            "Found {} related content items across sessions for topic: '{}'",
            filtered.len(),
            topic
        );

        Ok(filtered)
    }

    /// Compute semantic similarity between two text strings.
    /// Returns a similarity score between 0.0 and 1.0.
    pub async fn compute_text_similarity(&self, text1: &str, text2: &str) -> Result<f32> {
        let embedding1 = self.embedding_engine.encode_text(text1).await?;
        let embedding2 = self.embedding_engine.encode_text(text2).await?;

        let dot_product: f32 = embedding1
            .iter()
            .zip(embedding2.iter())
            .map(|(a, b)| a * b)
            .sum();
        let norm1: f32 = embedding1.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm2: f32 = embedding2.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm1 == 0.0 || norm2 == 0.0 {
            return Ok(0.0);
        }
        Ok(dot_product / (norm1 * norm2))
    }

    /// Get statistics about vectorized content (sizes, dimension).
    pub fn get_vectorization_stats(&self) -> HashMap<String, usize> {
        let db_stats = self.vector_db.get_stats();

        let mut stats = HashMap::new();
        stats.insert("total_vectors".to_string(), db_stats.total_vectors);
        stats.insert(
            "memory_usage_mb".to_string(),
            db_stats.memory_usage_bytes / 1024 / 1024,
        );
        stats.insert(
            "embedding_dimension".to_string(),
            self.embedding_engine.embedding_dimension(),
        );

        stats
    }

    /// Check whether a session has UPDATE embeddings (not just entities) — used to
    /// drive auto-vectorization.
    pub fn is_session_vectorized(&self, session_id: Uuid) -> bool {
        self.vector_db
            .has_session_update_embeddings(&session_id.to_string())
    }

    /// Count embeddings for a specific session.
    pub fn count_session_embeddings(&self, session_id: Uuid) -> usize {
        self.vector_db
            .count_session_embeddings(&session_id.to_string())
    }
}

fn parse_content_type(content_type_str: &str) -> ContentType {
    match content_type_str {
        "EntityDescription" => ContentType::EntityDescription,
        "UserMessage" => ContentType::UserMessage,
        "DecisionPoint" => ContentType::DecisionPoint,
        "CodeSnippet" => ContentType::CodeSnippet,
        "ProblemSolution" => ContentType::ProblemSolution,
        "SessionMetadata" => ContentType::SessionMetadata,
        _ => ContentType::UpdateContent,
    }
}
