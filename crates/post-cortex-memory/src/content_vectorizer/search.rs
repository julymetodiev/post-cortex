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
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::{debug, warn};
use uuid::Uuid;

use post_cortex_core::graph::retrieval_graph::{self, GraphRecord};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::{
    Corpus, MESSAGE_CONTENT_TYPE, Reranker, SearchMatch, VectorMetadata, cosine_similarity,
};

use crate::query_cache::CacheScope;

use super::types::{
    AUTHOR_KEY, CHUNK_PARENT_KEY, ContentType, PREFER_AUTHOR_BOOST, SearchMode, SearchOptions,
    SemanticSearchResult,
};
use super::vectorizer::ContentVectorizer;

/// Candidates taken from each ranking (vector and BM25) before fusion. Short
/// lists fused best in the retrieval prototype (LoCoMo all@50: 0.709 fusing
/// the top 50 of each, 0.691 the top 100, 0.662 the top 200).
const FUSION_DEPTH: usize = 50;

/// Candidates the reranker scores, taken from each ranking and from their
/// fusion. A cross-encoder over a deeper pool keeps gaining: LoCoMo all@50
/// with ±1 neighbours 0.878 reranking the top 300, 0.854 the top 200, 0.823
/// the top 100.
pub(crate) const RERANK_POOL: usize = 300;

/// Reciprocal rank fusion constant.
const RRF_K: f32 = 60.0;

type DateRange = Option<(DateTime<Utc>, DateTime<Utc>)>;

/// What a search keeps of the vectors of its sessions.
#[derive(Clone, Copy)]
struct Filter<'a> {
    corpus: Corpus,
    date_range: DateRange,
    /// Only vectors with this author (`AUTHOR_KEY` in their metadata).
    author: Option<&'a str>,
}

impl<'a> Filter<'a> {
    fn of(options: &'a SearchOptions) -> Self {
        Self {
            corpus: options.corpus,
            date_range: options.date_range,
            author: options.author.as_deref(),
        }
    }

    /// Whether `metadata` passes: in the corpus, dated within the range, by
    /// the author.
    fn admits(&self, metadata: &VectorMetadata) -> bool {
        self.corpus.admits(&metadata.content_type)
            && self
                .date_range
                .is_none_or(|(start, end)| metadata.timestamp >= start && metadata.timestamp <= end)
            && self
                .author
                .is_none_or(|a| metadata.metadata.get(AUTHOR_KEY).is_some_and(|m| m == a))
    }

    /// Whether the vector DB has vectors this filter drops, so a search has
    /// to apply it.
    fn is_restrictive(&self, db: &post_cortex_embeddings::VectorDB) -> bool {
        self.date_range.is_some() || self.author.is_some() || db.needs_corpus_filter(self.corpus)
    }
}

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
        Ok(self
            .semantic_search_shared(query, limit, session_filter, options)
            .await?
            .to_vec())
    }

    /// [`Self::semantic_search`] returning the results shared with the query
    /// cache: a cache hit costs one refcount bump instead of a deep copy.
    ///
    /// # Errors
    /// Returns an error if the query cannot be embedded or the vector database search fails.
    pub async fn semantic_search_shared(
        &self,
        query: &str,
        limit: usize,
        session_filter: Option<Uuid>,
        options: SearchOptions,
    ) -> Result<Arc<[SemanticSearchResult]>> {
        let recency_bias = options.recency_bias.unwrap_or(self.config.recency_bias);

        debug!("Performing semantic search for: '{}'", query);

        if session_filter.is_none() && !self.config.enable_cross_session_search {
            return Ok(Arc::from(Vec::new()));
        }

        let (scope_tag, scope) = match session_filter {
            Some(session_id) => (1u8, CacheScope::Session(session_id)),
            None => (0u8, CacheScope::Global),
        };
        let filter = Filter::of(&options);
        let (mode, reranker) = self.reranker_for(options.mode);
        let params_hash = params_hash(
            query,
            limit,
            scope_tag,
            session_filter.as_slice(),
            recency_bias,
            mode,
            filter,
            options.prefer_author.as_deref(),
        );

        // Check the cache before paying for the embedding.
        if let Some(ref cache) = self.query_cache
            && let Some(cached) = cache.get(params_hash, query)
        {
            debug!("Cache hit for semantic search query: '{}'", query);
            return Ok(cached);
        }

        let query_embedding = self.embedding_engine.encode_text(query).await?;

        let sources: Vec<String> = session_filter.iter().map(Uuid::to_string).collect();
        let lexical = self
            .lexical_search(
                query,
                &query_embedding,
                limit,
                &sources,
                filter,
                reranker.is_some(),
            )
            .await;
        let depth = search_depth(limit, lexical.as_ref(), reranker.is_some());

        let filtered = filter.is_restrictive(&self.vector_db);
        let keep = |metadata: &VectorMetadata| filter.admits(metadata);
        let search_results = match (session_filter, filtered) {
            (Some(session_id), true) => self.vector_db.search_in_sources_with_filter(
                &query_embedding,
                depth,
                &[session_id.to_string()],
                keep,
            )?,
            (Some(session_id), false) => {
                self.vector_db
                    .search_in_source(&query_embedding, depth, &session_id.to_string())?
            }
            (None, true) => self
                .vector_db
                .search_with_filter(&query_embedding, depth, keep)?,
            (None, false) => self.vector_db.search(&query_embedding, depth)?,
        };
        // The retrieval graph links records only.
        let graph = match (
            reranker,
            session_filter,
            options.graph_candidates.as_deref(),
        ) {
            (Some(_), Some(session_id), Some(ids)) if filter.corpus != Corpus::Messages => {
                Some(self.graph_matches(ids, &query_embedding, &session_id.to_string(), filter))
            }
            _ => None,
        };

        let ranked = rerank(reranker, query, rank(search_results, [lexical, graph])).await;
        let mut results = self.process_search_results(
            ranked,
            Some(recency_bias),
            options.prefer_author.as_deref(),
        )?;
        results.truncate(limit);
        let results: Arc<[SemanticSearchResult]> = Arc::from(results);

        if let Some(ref cache) = self.query_cache {
            cache.insert(query.to_string(), Arc::clone(&results), params_hash, scope);
        }

        Ok(results)
    }

    /// Perform semantic search within a specific set of sessions.
    ///
    /// Scores only the vectors of the allowed sessions (via the source index)
    /// in a single pass, then applies optional scoring adjustments (recency
    /// bias, etc.) in post-processing.
    pub async fn semantic_search_multisession(
        &self,
        query: &str,
        limit: usize,
        allowed_sessions: &[Uuid],
        options: SearchOptions,
    ) -> Result<Vec<SemanticSearchResult>> {
        Ok(self
            .semantic_search_multisession_shared(query, limit, allowed_sessions, options)
            .await?
            .to_vec())
    }

    /// [`Self::semantic_search_multisession`] returning results shared with
    /// the query cache.
    ///
    /// # Errors
    /// Returns an error if the query cannot be embedded or the vector database search fails.
    pub async fn semantic_search_multisession_shared(
        &self,
        query: &str,
        limit: usize,
        allowed_sessions: &[Uuid],
        options: SearchOptions,
    ) -> Result<Arc<[SemanticSearchResult]>> {
        let recency_bias = options.recency_bias.unwrap_or(self.config.recency_bias);

        debug!(
            "Performing multisession semantic search with recency_bias={:?} for: '{}'",
            recency_bias, query
        );

        // Order and duplicates don't change the result set; normalise so they
        // share one cache entry.
        let mut sessions = allowed_sessions.to_vec();
        sessions.sort_unstable();
        sessions.dedup();

        let filter = Filter::of(&options);
        let (mode, reranker) = self.reranker_for(options.mode);
        let params_hash = params_hash(
            query,
            limit,
            2,
            &sessions,
            recency_bias,
            mode,
            filter,
            options.prefer_author.as_deref(),
        );

        if let Some(ref cache) = self.query_cache
            && let Some(cached) = cache.get(params_hash, query)
        {
            debug!(
                "Cache hit for multisession semantic search query: '{}'",
                query
            );
            return Ok(cached);
        }

        let query_embedding = self.embedding_engine.encode_text(query).await?;

        let sources: Vec<String> = sessions.iter().map(Uuid::to_string).collect();
        let lexical = self
            .lexical_search(
                query,
                &query_embedding,
                limit,
                &sources,
                filter,
                reranker.is_some(),
            )
            .await;
        let depth = search_depth(limit, lexical.as_ref(), reranker.is_some());
        let search_results = if filter.is_restrictive(&self.vector_db) {
            self.vector_db.search_in_sources_with_filter(
                &query_embedding,
                depth,
                &sources,
                |metadata| filter.admits(metadata),
            )?
        } else {
            self.vector_db
                .search_in_sources(&query_embedding, depth, &sources)?
        };

        let ranked = rerank(reranker, query, rank(search_results, [lexical, None])).await;
        let mut results = self.process_search_results(
            ranked,
            Some(recency_bias),
            options.prefer_author.as_deref(),
        )?;
        results.truncate(limit);
        let results: Arc<[SemanticSearchResult]> = Arc::from(results);

        if let Some(ref cache) = self.query_cache {
            cache.insert(
                query.to_string(),
                Arc::clone(&results),
                params_hash,
                CacheScope::Sessions(Arc::from(sessions)),
            );
        }

        Ok(results)
    }

    /// BM25 hits over the same sessions (`sources`; empty means all) that
    /// pass `filter`, for the query's words that are not too common
    /// ([`VectorDB::query_terms`](post_cortex_embeddings::VectorDB::query_terms)):
    /// from the storage backend's full-text index (SurrealDB), else from the
    /// vector DB's in-memory one (RocksDB). `None` when neither has one, no
    /// word is left or the query failed; search then uses vectors only.
    async fn lexical_search(
        &self,
        query: &str,
        query_embedding: &[f32],
        limit: usize,
        sources: &[String],
        filter: Filter<'_>,
        reranking: bool,
    ) -> Option<Vec<SearchMatch>> {
        let terms = self.vector_db.query_terms(query);
        if terms.is_empty() {
            return None;
        }
        let depth = limit.max(pool_depth(reranking));
        let from_storage = match &self.persistent_storage {
            Some(storage) => storage
                .search_text(
                    &terms.join(" "),
                    query_embedding,
                    depth,
                    sources,
                    filter.corpus,
                )
                .await
                .unwrap_or_else(|e| {
                    debug!("Full-text search failed, using vectors only: {:#}", e);
                    None
                }),
            None => None,
        };
        let mut hits = from_storage.or_else(|| {
            self.vector_db
                .search_text(&terms, query_embedding, depth, sources, filter.corpus)
        })?;
        hits.retain(|m| filter.admits(&m.metadata));
        Some(hits)
    }

    /// Whether a search in `mode` is reranked (and so takes graph
    /// candidates, see [`SearchOptions::graph_candidates`]).
    pub fn reranks(&self, mode: Option<SearchMode>) -> bool {
        self.reranker_for(mode).1.is_some()
    }

    /// The session's update vectors as retrieval-graph records: content id,
    /// the terms and entity names stored in the vector's metadata. Vectors
    /// stored before terms were get them extracted from their text.
    pub fn graph_records(&self, session_id: Uuid) -> Vec<GraphRecord<String>> {
        self.vector_db
            .source_vector_ids(&session_id.to_string())
            .into_iter()
            .filter_map(|id| self.vector_db.get_metadata(id))
            // Updates, not chunks: the first chunk carries the whole update's
            // terms.
            .filter(|m| {
                m.content_type != "EntityDescription"
                    && m.content_type != MESSAGE_CONTENT_TYPE
                    && !m.metadata.contains_key(CHUNK_PARENT_KEY)
            })
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
            .collect()
    }

    /// Vector-store matches for the content ids the retrieval graph ranked,
    /// in its order, limited to `source` and `filter`.
    fn graph_matches(
        &self,
        ids: &[String],
        query_embedding: &[f32],
        source: &str,
        filter: Filter<'_>,
    ) -> Vec<SearchMatch> {
        let query_norm = post_cortex_embeddings::l2_norm(query_embedding);
        ids.iter()
            .filter_map(|id| {
                let vector_id = self.vector_db.find_vector_id_by_content_id(id)?;
                let metadata = self.vector_db.get_metadata(vector_id)?;
                (metadata.source == source && filter.admits(&metadata)).then(|| SearchMatch {
                    vector_id,
                    similarity: self
                        .vector_db
                        .similarity_to(query_embedding, query_norm, vector_id)
                        .unwrap_or(0.0),
                    metadata,
                })
            })
            .collect()
    }

    /// The mode a search runs in and the reranker it uses. `mode` None means
    /// the configured default, else the best mode the rerankers allow.
    fn reranker_for(&self, mode: Option<SearchMode>) -> (SearchMode, Option<&Arc<dyn Reranker>>) {
        let mode = mode
            .or(self.default_mode)
            .unwrap_or(if self.accurate_reranker.is_some() {
                SearchMode::Accurate
            } else if self.reranker.is_some() {
                SearchMode::Local
            } else {
                SearchMode::Fast
            });
        let reranker = match mode {
            SearchMode::Fast => None,
            SearchMode::Local => self.reranker.as_ref(),
            SearchMode::Accurate => self.accurate_reranker.as_ref().or(self.reranker.as_ref()),
        };
        (mode, reranker)
    }

    /// Turn ranked matches into semantic results. Each match comes with its
    /// relevance in [0, 1] (cosine similarity, or the fused score), which is
    /// what the combined score ranks by; `similarity_score` stays the cosine.
    /// Results by `prefer_author` get [`PREFER_AUTHOR_BOOST`] on top.
    fn process_search_results(
        &self,
        search_results: Vec<(SearchMatch, f32)>,
        recency_bias_override: Option<f32>,
        prefer_author: Option<&str>,
    ) -> Result<Vec<SemanticSearchResult>> {
        let now = Utc::now();
        // Defense-in-depth: clamp recency_bias to prevent negative or extreme values
        let recency_bias = recency_bias_override
            .unwrap_or(self.config.recency_bias)
            .clamp(0.0, 10.0);

        let score_adjuster: Option<Box<dyn crate::scoring::ScoreAdjuster>> = if recency_bias > 0.0 {
            Some(Box::new(crate::scoring::TemporalDecayAdjuster::new(
                recency_bias,
                now,
            )))
        } else {
            None
        };

        let decay_start = std::time::Instant::now();
        let mut decay_count = 0;

        let mut results = Vec::new();
        for (result, relevance) in search_results {
            let content_type = parse_content_type(&result.metadata.content_type);
            let session_id = Uuid::parse_str(&result.metadata.source)
                .context("Invalid session ID in metadata")?;

            let importance_weight = content_type.importance_weight();
            let base_score = relevance.mul_add(0.7, importance_weight * 0.3);

            let mut combined_score = if let Some(ref adjuster) = score_adjuster {
                decay_count += 1;
                adjuster.adjust(base_score, &result.metadata)
            } else {
                base_score
            };
            let author = result.metadata.metadata.get(AUTHOR_KEY).cloned();
            if prefer_author.is_some() && author.as_deref() == prefer_author {
                combined_score += PREFER_AUTHOR_BOOST;
            }

            // A chunk of a long update answers for the update.
            let content_id = match result.metadata.metadata.get(CHUNK_PARENT_KEY) {
                Some(parent) => parent.clone(),
                None => result.metadata.id,
            };
            results.push(SemanticSearchResult {
                content_id,
                session_id,
                content_type,
                text_content: result.metadata.text,
                similarity_score: result.similarity,
                importance_score: importance_weight,
                timestamp: result.metadata.timestamp,
                combined_score,
                neighbour_of: None,
                author,
                derived_from: Vec::new(),
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
            .semantic_search_shared(topic, limit * 2, None, SearchOptions::default())
            .await?;

        let filtered: Vec<_> = results
            .iter()
            .filter(|r| r.session_id != session.id())
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

    /// Compute semantic similarity between two text strings.
    /// Returns a similarity score between 0.0 and 1.0.
    pub async fn compute_text_similarity(&self, text1: &str, text2: &str) -> Result<f32> {
        let embedding1 = self.embedding_engine.encode_text(text1).await?;
        let embedding2 = self.embedding_engine.encode_text(text2).await?;

        Ok(cosine_similarity(&embedding1, &embedding2))
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

/// Candidates taken from each ranking: the reranker's pool when the search
/// is reranked, else the depth that fuses best on its own.
fn pool_depth(reranking: bool) -> usize {
    if reranking { RERANK_POOL } else { FUSION_DEPTH }
}

/// How many vector candidates to fetch: deeper when they will be fused with
/// BM25 hits or reranked.
fn search_depth(limit: usize, lexical: Option<&Vec<SearchMatch>>, reranking: bool) -> usize {
    match lexical {
        _ if reranking => limit.max(RERANK_POOL),
        Some(hits) if !hits.is_empty() => limit.max(FUSION_DEPTH),
        _ => limit,
    }
}

/// Reorder the best [`RERANK_POOL`] candidates by the reranker's score;
/// the rest are dropped. The reranker's relevance replaces the fused score. If
/// scoring fails the fused order stands.
async fn rerank(
    reranker: Option<&Arc<dyn Reranker>>,
    query: &str,
    mut ranked: Vec<(SearchMatch, f32)>,
) -> Vec<(SearchMatch, f32)> {
    let Some(reranker) = reranker else {
        return ranked;
    };
    ranked.truncate(RERANK_POOL);
    let documents: Vec<String> = ranked
        .iter()
        .map(|(m, _)| m.metadata.text.clone())
        .collect();
    let scores = match reranker.score(query, &documents).await {
        Ok(scores) if scores.len() == ranked.len() => scores,
        Ok(scores) => {
            warn!(
                "Reranker returned {} scores for {} candidates, keeping the fused order",
                scores.len(),
                ranked.len()
            );
            return ranked;
        }
        Err(e) => {
            warn!("Reranking failed, keeping the fused order: {:#}", e);
            return ranked;
        }
    };
    let mut reranked: Vec<(SearchMatch, f32)> = ranked
        .into_iter()
        .zip(scores)
        .map(|((m, _), relevance)| (m, relevance))
        .collect();
    reranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    reranked
}

/// Pair each match with the relevance it ranks by: the cosine similarity for
/// vector search alone, the fused score when other rankings (BM25, the
/// retrieval graph) have candidates.
fn rank<const N: usize>(
    vector: Vec<SearchMatch>,
    others: [Option<Vec<SearchMatch>>; N],
) -> Vec<(SearchMatch, f32)> {
    let others: Vec<Vec<SearchMatch>> = others
        .into_iter()
        .flatten()
        .filter(|list| !list.is_empty())
        .collect();
    if others.is_empty() {
        return vector
            .into_iter()
            .map(|m| {
                let similarity = m.similarity;
                (m, similarity)
            })
            .collect();
    }
    fuse(std::iter::once(vector).chain(others).collect())
}

/// Reciprocal rank fusion of the rankings, best first. Records are matched
/// by (session, content id); the copy from the earliest list is kept (the
/// vector one first). Adjacent records with the same text share a rank.
/// Scores are normalised so that ranking first in every list gives 1.0.
fn fuse(lists: Vec<Vec<SearchMatch>>) -> Vec<(SearchMatch, f32)> {
    let best = lists.len() as f32 / (RRF_K + 1.0);
    let mut fused: HashMap<(String, String), (SearchMatch, f32)> = HashMap::new();
    for list in lists {
        let mut position = 0usize;
        let mut rank = 0usize;
        let mut previous_text: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        for m in list {
            let key = (m.metadata.source.clone(), m.metadata.id.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            // Records with the same text score the same in every ranking;
            // they share a rank so the tie survives fusion.
            if previous_text.as_deref() != Some(m.metadata.text.as_str()) {
                rank = position;
                previous_text = Some(m.metadata.text.clone());
            }
            position += 1;
            let score = 1.0 / (RRF_K + rank as f32 + 1.0);
            fused
                .entry(key)
                .and_modify(|(_, s)| *s += score)
                .or_insert((m, score));
        }
    }
    let mut out: Vec<(SearchMatch, f32)> =
        fused.into_values().map(|(m, s)| (m, s / best)).collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// Cache key over every parameter that changes the result set.
/// `scope_tag` keeps global, single-session and multi-session keys apart.
fn params_hash(
    query: &str,
    limit: usize,
    scope_tag: u8,
    sessions: &[Uuid],
    recency_bias: f32,
    mode: SearchMode,
    filter: Filter<'_>,
    prefer_author: Option<&str>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    query.hash(&mut hasher);
    limit.hash(&mut hasher);
    scope_tag.hash(&mut hasher);
    sessions.hash(&mut hasher);
    if let Some((start, end)) = filter.date_range {
        start.timestamp().hash(&mut hasher);
        end.timestamp().hash(&mut hasher);
    }
    // Include recency_bias in cache key to prevent collisions
    if recency_bias != 0.0 {
        recency_bias.to_bits().hash(&mut hasher);
    }
    mode.hash(&mut hasher);
    filter.corpus.hash(&mut hasher);
    filter.author.hash(&mut hasher);
    prefer_author.hash(&mut hasher);
    hasher.finish()
}

fn parse_content_type(content_type_str: &str) -> ContentType {
    match content_type_str {
        "EntityDescription" => ContentType::EntityDescription,
        "UserMessage" => ContentType::UserMessage,
        "DecisionPoint" => ContentType::DecisionPoint,
        "CodeSnippet" => ContentType::CodeSnippet,
        "ProblemSolution" => ContentType::ProblemSolution,
        "SessionMetadata" => ContentType::SessionMetadata,
        MESSAGE_CONTENT_TYPE => ContentType::Message,
        _ => ContentType::UpdateContent,
    }
}
