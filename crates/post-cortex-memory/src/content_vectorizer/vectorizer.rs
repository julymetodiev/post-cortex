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

//! `ContentVectorizer` — the orchestrator type: the embedding engine and
//! the search index it embeds and retrieves through, the rerankers, the
//! query cache and what it knows is vectorized.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::query_cache::QueryCache;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::{EmbeddingEngine, IndexEntry, LoadReport, Reranker, SearchIndex};

use crate::content_vectorizer::types::{ContentVectorizerConfig, SearchMode};

/// Main content vectorization pipeline
#[derive(Clone)]
pub struct ContentVectorizer {
    engine: Arc<dyn EmbeddingEngine>,
    index: Arc<dyn SearchIndex>,
    config: ContentVectorizerConfig,
    query_cache: Option<Arc<QueryCache>>,
    /// Local reranker (`local` mode)
    reranker: Option<Arc<dyn Reranker>>,
    /// Remote reranker, with the local one as fallback (`accurate` mode)
    accurate_reranker: Option<Arc<dyn Reranker>>,
    /// Mode for searches that do not name one; None = the best available
    default_mode: Option<SearchMode>,
    /// Recency bias performance metrics — all atomic for lock-free access,
    /// shared between clones for aggregation
    recency_bias_total_duration_ns: Arc<AtomicU64>,
    recency_bias_total_results: Arc<AtomicU64>,
    recency_bias_calculation_count: Arc<AtomicU64>,
    /// Per session: the (update count, entity count) at the last complete
    /// `vectorize_session`. Lets searches skip re-vectorizing a session
    /// that has nothing new — including sessions whose updates are all too
    /// short to embed, which never get update vectors.
    vectorized_marks: Arc<DashMap<Uuid, (usize, usize)>>,
    /// What the index loaded when the vectorizer started (for health)
    load_report: Option<LoadReport>,
}

impl ContentVectorizer {
    /// A vectorizer embedding with `engine` into `index`.
    pub fn new(
        config: ContentVectorizerConfig,
        engine: Arc<dyn EmbeddingEngine>,
        index: Arc<dyn SearchIndex>,
    ) -> Self {
        let query_cache = config
            .enable_query_caching
            .then(|| Arc::new(QueryCache::new(config.query_cache_config.clone())));
        debug!(
            "ContentVectorizer initialized with query caching: {}",
            config.enable_query_caching
        );
        Self {
            engine,
            index,
            config,
            query_cache,
            reranker: None,
            accurate_reranker: None,
            default_mode: None,
            recency_bias_total_duration_ns: Arc::new(AtomicU64::new(0)),
            recency_bias_total_results: Arc::new(AtomicU64::new(0)),
            recency_bias_calculation_count: Arc::new(AtomicU64::new(0)),
            vectorized_marks: Arc::new(DashMap::new()),
            load_report: None,
        }
    }

    /// Record what the index loaded when the vectorizer started.
    pub fn set_load_report(&mut self, report: LoadReport) {
        self.load_report = Some(report);
    }

    /// What the index loaded when the vectorizer started.
    pub fn load_report(&self) -> Option<LoadReport> {
        self.load_report.clone()
    }

    /// The engine this vectorizer embeds with.
    pub fn embedding_engine(&self) -> &Arc<dyn EmbeddingEngine> {
        &self.engine
    }

    /// The index this vectorizer stores and retrieves through.
    pub fn index(&self) -> &Arc<dyn SearchIndex> {
        &self.index
    }

    /// The vectorizer's configuration.
    pub fn config(&self) -> &ContentVectorizerConfig {
        &self.config
    }

    /// The query result cache, when caching is on.
    pub fn query_cache(&self) -> Option<&Arc<QueryCache>> {
        self.query_cache.as_ref()
    }

    /// The mode a search runs in and the reranker it uses. `mode` None means
    /// the configured default, else the best mode the rerankers allow. An
    /// accurate search uses the accurate reranker (the remote one) only when
    /// `remote` allows it, else the local one, if any.
    pub fn reranker_for(
        &self,
        mode: Option<SearchMode>,
        remote: bool,
    ) -> (SearchMode, Option<&Arc<dyn Reranker>>) {
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
            SearchMode::Accurate => self
                .accurate_reranker
                .as_ref()
                .filter(|_| remote)
                .or(self.reranker.as_ref()),
        };
        (mode, reranker)
    }

    /// Whether a search in `mode` would use the remote reranker if allowed:
    /// it runs accurate and one is configured.
    pub fn would_rerank_remotely(&self, mode: Option<SearchMode>) -> bool {
        self.accurate_reranker.is_some() && self.reranker_for(mode, true).0 == SearchMode::Accurate
    }

    /// Count one recency-bias pass over `results` results that took
    /// `duration_ns`.
    pub fn record_recency_pass(&self, duration_ns: u64, results: u64) {
        self.recency_bias_total_duration_ns
            .fetch_add(duration_ns, Ordering::Relaxed);
        self.recency_bias_total_results
            .fetch_add(results, Ordering::Relaxed);
        self.recency_bias_calculation_count
            .fetch_add(1, Ordering::Relaxed);
    }

    /// The recency-bias totals: time in ns, results, passes.
    pub fn recency_totals(&self) -> (u64, u64, u64) {
        (
            self.recency_bias_total_duration_ns.load(Ordering::Relaxed),
            self.recency_bias_total_results.load(Ordering::Relaxed),
            self.recency_bias_calculation_count.load(Ordering::Relaxed),
        )
    }

    /// Forget that `session_id` was fully vectorized (its vectors are gone).
    pub fn forget_vectorized(&self, session_id: Uuid) {
        self.vectorized_marks.remove(&session_id);
    }

    /// The local reranker used in `local` mode (e.g. a cross-encoder).
    pub fn set_reranker(&mut self, reranker: Arc<dyn Reranker>) {
        self.reranker = Some(reranker);
    }

    /// The reranker used in `accurate` mode (e.g. Voyage with the local one
    /// as fallback).
    pub fn set_accurate_reranker(&mut self, reranker: Arc<dyn Reranker>) {
        self.accurate_reranker = Some(reranker);
    }

    /// Mode for searches that do not name one. Unset, it is the best mode
    /// the configured rerankers allow.
    pub fn set_default_mode(&mut self, mode: SearchMode) {
        self.default_mode = Some(mode);
    }

    /// Store `entries` in the index in one batch. Each replaces an earlier
    /// entry of the same (session, content id), so storing again (a retry,
    /// a job racing another for the same record) leaves one copy and
    /// nothing has to be rolled back. Returns the content ids stored; none
    /// when the batch failed, so the caller does not mark them vectorized.
    pub async fn store_vectors(&self, entries: Vec<IndexEntry>) -> Vec<String> {
        if entries.is_empty() {
            return Vec::new();
        }
        let content_ids: Vec<String> = entries.iter().map(|e| e.meta.id.clone()).collect();
        if let Err(e) = self.index.upsert(entries).await {
            warn!(
                "Failed to store {} vector(s) (first {}): {:#}. NOT marking as vectorized.",
                content_ids.len(),
                content_ids.first().map_or("", String::as_str),
                e
            );
            return Vec::new();
        }
        content_ids
    }

    /// Drop cached query results that a change to `session_id`'s vectors
    /// can affect (its own, multi-session and global entries).
    pub fn invalidate_cached(&self, session_id: Uuid) {
        if let Some(cache) = &self.query_cache {
            cache.invalidate_session(session_id);
        }
    }

    /// Whether `vectorize_session` has work to do for this session: false
    /// only when the last complete run saw the same update and entity counts.
    pub fn needs_vectorization(&self, session: &ActiveSession) -> bool {
        self.vectorized_marks
            .get(&session.id())
            .is_none_or(|mark| *mark != vectorization_mark(session))
    }

    /// Remember that `session` (this snapshot) is fully vectorized.
    pub fn mark_vectorized(&self, session: &ActiveSession) {
        self.vectorized_marks
            .insert(session.id(), vectorization_mark(session));
    }
}

/// Snapshot of what a complete `vectorize_session` covered.
fn vectorization_mark(session: &ActiveSession) -> (usize, usize) {
    (
        session.records().len(),
        session.entity_graph().entity_count(),
    )
}
