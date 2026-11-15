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

use crate::content_vectorizer::ContentVectorizer;
use crate::content_vectorizer::fusion::note_fused;
use crate::content_vectorizer::fusion::pool_depth;
use crate::content_vectorizer::fusion::rank;
use crate::content_vectorizer::fusion::rerank;
use crate::content_vectorizer::fusion::search_depth;
use crate::content_vectorizer::fusion::traces;
use crate::content_vectorizer::types::{SearchMode, SearchOptions, SemanticSearchResult};
use crate::query_cache::CacheScope;
use post_cortex_ports::error::Result;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

use post_cortex_ports::{Corpus, IndexFilter, IndexHit, PortError};

/// Which vectors a search scores.
#[derive(Debug, Clone, Copy)]
pub enum VectorScope<'a> {
    /// Every session's (empty when cross-session search is off).
    All,
    /// Those of these sessions; order and duplicates do not matter.
    Sessions(&'a [Uuid]),
}

impl ContentVectorizer {
    /// The one semantic search: vector, BM25 and (for one session searched
    /// with [`SearchOptions::graph_candidates`] and a reranker) retrieval
    /// graph rankings fused, reranked, scored and cut to `limit`. The
    /// results are shared with the query cache: a hit costs one refcount
    /// bump.
    ///
    /// # Errors
    /// Returns an error if the query cannot be embedded or the vector database search fails.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        scope: VectorScope<'_>,
        options: SearchOptions,
    ) -> Result<Arc<[SemanticSearchResult]>> {
        let recency_bias = options.recency_bias.unwrap_or(self.config().recency_bias);
        debug!("Performing semantic search for: '{}'", query);

        // Order and duplicates don't change the result set; normalise so they
        // share one cache entry.
        let sessions: Option<Vec<Uuid>> = match scope {
            VectorScope::All if !self.config().enable_cross_session_search => {
                return Ok(Arc::from(Vec::new()));
            }
            VectorScope::All => None,
            // No session: nothing to find (BM25 would read "no source" as
            // every source).
            VectorScope::Sessions([]) => return Ok(Arc::from(Vec::new())),
            VectorScope::Sessions(ids) => {
                let mut ids = ids.to_vec();
                ids.sort_unstable();
                ids.dedup();
                Some(ids)
            }
        };
        let (scope_tag, cache_scope) = match sessions.as_deref() {
            None => (0u8, CacheScope::Global),
            Some([one]) => (1, CacheScope::Session(*one)),
            Some(many) => (2, CacheScope::Sessions(Arc::from(many))),
        };
        let filter = IndexFilter {
            sessions: sessions
                .as_ref()
                .map(|ids| ids.iter().map(Uuid::to_string).collect()),
            corpus: options.corpus,
            date_range: options.date_range,
            author: options.author.clone(),
        };
        let (mode, reranker) = self.reranker_for(options.mode, options.remote_rerank);
        // The retrieval graph links the records of one session only.
        let graph_candidates = match (reranker, sessions.as_deref()) {
            (Some(_), Some([_])) if filter.corpus != Corpus::Messages => {
                options.graph_candidates.as_deref()
            }
            _ => None,
        };
        let params_hash = CacheKey {
            query,
            limit,
            scope_tag,
            sessions: sessions.as_deref().unwrap_or_default(),
            recency_bias,
            mode,
            remote: options.remote_rerank,
            filter: &filter,
            prefer_author: options.prefer_author.as_deref(),
            graph_candidates,
        }
        .digest();

        // Check the cache before paying for the embedding.
        if let Some(cache) = self.query_cache()
            && !options.trace
            && let Some(cached) = cache.get(params_hash, query)
        {
            debug!("Cache hit for semantic search query: '{}'", query);
            return Ok(cached);
        }

        let query_embedding = self
            .embedding_engine()
            .embed(vec![query.to_string()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| PortError::Unavailable("No embeddings generated".into()))?;
        let lexical = self
            .lexical_search(query, &query_embedding, limit, &filter, reranker.is_some())
            .await;
        let depth = search_depth(limit, lexical.as_ref(), reranker.is_some());

        let search_results = self
            .index()
            .nearest(&query_embedding, &filter, depth)
            .await?;
        let graph = match (graph_candidates, filter.sessions.as_deref()) {
            (Some(ids), Some([session])) => Some(
                self.graph_matches(ids, &query_embedding, session, &filter)
                    .await?,
            ),
            _ => None,
        };

        let mut traces = options
            .trace
            .then(|| traces(&search_results, lexical.as_ref(), graph.as_ref()));
        let fused = rank(search_results, [lexical, graph]);
        let ranked = rerank(reranker, query, note_fused(fused, traces.as_mut())).await;
        let mut results = crate::content_vectorizer::results::process_search_results(
            self,
            ranked,
            recency_bias,
            options.prefer_author.as_deref(),
            traces.as_ref().map(|t| (t, reranker.is_some())),
        )?;
        results.truncate(limit);
        let results: Arc<[SemanticSearchResult]> = Arc::from(results);

        if let Some(cache) = self.query_cache()
            && !options.trace
        {
            cache.insert(
                query.to_string(),
                Arc::clone(&results),
                params_hash,
                cache_scope,
            );
        }
        Ok(results)
    }

    /// BM25 hits within `filter` for the query's words that are not too
    /// common (the index picks them), each with its cosine similarity to
    /// the query. `None` when the lexical ranking failed; search then uses
    /// vectors only.
    async fn lexical_search(
        &self,
        query: &str,
        query_embedding: &[f32],
        limit: usize,
        filter: &IndexFilter,
        reranking: bool,
    ) -> Option<Vec<IndexHit>> {
        let depth = limit.max(pool_depth(reranking));
        let mut hits = self
            .index()
            .lexical(query, filter, depth)
            .await
            .map_err(|e| debug!("Lexical search failed, using vectors only: {:#}", e))
            .ok()?;
        // The similarity of each hit, scored per session.
        let mut by_session: HashMap<&str, Vec<String>> = HashMap::new();
        for hit in &hits {
            by_session
                .entry(hit.meta.source.as_str())
                .or_default()
                .push(hit.meta.id.clone());
        }
        let mut similarity: HashMap<(String, String), f32> = HashMap::new();
        for (session, ids) in by_session {
            let scored = self
                .index()
                .score(query_embedding, session, &ids)
                .await
                .map_err(|e| debug!("Lexical hits not scored, using vectors only: {:#}", e))
                .ok()?;
            for hit in scored {
                similarity.insert(
                    (hit.meta.source.clone(), hit.meta.id.clone()),
                    hit.similarity,
                );
            }
        }
        for hit in &mut hits {
            let key = (hit.meta.source.clone(), hit.meta.id.clone());
            hit.similarity = similarity.get(&key).copied().unwrap_or(0.0);
        }
        Some(hits)
    }

    /// Whether a search in `mode` is reranked (and so takes graph
    /// candidates, see [`SearchOptions::graph_candidates`]), with the remote
    /// reranker allowed when `remote`.
    pub fn reranks(&self, mode: Option<SearchMode>, remote: bool) -> bool {
        self.reranker_for(mode, remote).1.is_some()
    }
}

/// Every parameter that changes the result set of a search: the cache key.
/// `scope_tag` keeps global, single-session and multi-session keys apart,
/// `remote` searches that may use the remote reranker from those that may
/// not;
/// the graph candidates stand for the retrieval graph they came from, so a
/// rebuilt graph is a new key.
struct CacheKey<'a> {
    query: &'a str,
    limit: usize,
    scope_tag: u8,
    sessions: &'a [Uuid],
    recency_bias: f32,
    mode: SearchMode,
    remote: bool,
    filter: &'a IndexFilter,
    prefer_author: Option<&'a str>,
    graph_candidates: Option<&'a [String]>,
}

impl CacheKey<'_> {
    /// The key's hash.
    fn digest(&self) -> u64 {
        let CacheKey {
            query,
            limit,
            scope_tag,
            sessions,
            recency_bias,
            mode,
            remote,
            filter,
            prefer_author,
            graph_candidates,
        } = *self;
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
        remote.hash(&mut hasher);
        filter.corpus.hash(&mut hasher);
        filter.author.hash(&mut hasher);
        prefer_author.hash(&mut hasher);
        graph_candidates.hash(&mut hasher);
        hasher.finish()
    }
}
