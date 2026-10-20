use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[cfg(feature = "embeddings")]
use post_cortex_core::graph::retrieval_graph::RetrievalGraph;
use post_cortex_core::session::active_session::ActiveSession;

#[cfg(feature = "embeddings")]
use crate::content_vectorizer::{ContentVectorizer, ContentVectorizerConfig};
#[cfg(feature = "embeddings")]
use post_cortex_embeddings::EmbeddingConfig;
#[cfg(feature = "embeddings")]
use post_cortex_embeddings::VectorDbConfig;

use super::system::ConversationMemorySystem;

// Retry configuration constants
const MAX_VECTORIZATION_RETRIES: u32 = 3;
const VECTORIZATION_RETRY_DELAY_MS: u64 = 100;
const MAX_VECTORIZER_INIT_RETRIES: u32 = 10;

// Parallel processing constants
const MAX_PARALLEL_VECTORIZATION: usize = 4;
/// Results a semantic search returns when the caller sets no limit.
#[cfg(feature = "embeddings")]
const DEFAULT_SEARCH_LIMIT: usize = 20;

/// Retrieval graph per session, with the session's vector and relation
/// counts it was built from; rebuilt when either changes.
#[cfg(feature = "embeddings")]
pub(crate) type SessionGraphs = dashmap::DashMap<Uuid, (usize, usize, Arc<RetrievalGraph<String>>)>;
/// Tokens per (query, record) pair the cross-encoder sees; the retrieval
/// prototype was measured at 256.
#[cfg(all(feature = "embeddings", feature = "rerank"))]
const RERANKER_MAX_LENGTH: usize = 256;

/// Each document sent to Voyage is cut to about this many tokens: 31% fewer
/// tokens than whole records, no loss on LongMemEval.
#[cfg(all(feature = "embeddings", feature = "rerank-voyage"))]
const VOYAGE_MAX_DOC_TOKENS: usize = 256;

/// A Voyage call slower than this fails over to the local reranker.
#[cfg(all(feature = "embeddings", feature = "rerank-voyage"))]
const VOYAGE_TIMEOUT: Duration = Duration::from_secs(10);

impl ConversationMemorySystem {
    /// Set up the rerankers: `reranker_model` (local cross-encoder) serves
    /// `local` mode; Voyage, with the local model as fallback, serves
    /// `accurate` mode. Anything that fails to load is logged and left out,
    /// and searches in its mode fall back to a cheaper one rather than fail.
    #[cfg(feature = "embeddings")]
    async fn attach_reranker(&self, vectorizer: &mut ContentVectorizer) {
        use post_cortex_embeddings::Reranker;

        let holder = &self.embedding_config_holder;
        let local: Option<Arc<dyn Reranker>> = match &holder.reranker_model {
            Some(model) => self.load_local_reranker(model).await,
            None => None,
        };
        let remote: Option<Arc<dyn Reranker>> = match &holder.voyage {
            Some((key, model)) => self.load_voyage_reranker(key, model),
            None => None,
        };
        if let Some(local) = &local {
            vectorizer.set_reranker(Arc::clone(local));
        }
        if let Some(remote) = remote {
            let accurate: Arc<dyn Reranker> = match local {
                Some(local) => Arc::new(post_cortex_embeddings::rerank::FallbackReranker::new(
                    remote, local,
                )),
                None => remote,
            };
            vectorizer.set_accurate_reranker(accurate);
        }
        if let Some(mode) = &holder.search_mode {
            match mode.parse::<crate::content_vectorizer::SearchMode>() {
                Ok(mode) => vectorizer.set_default_mode(mode),
                Err(e) => warn!("Ignoring search_mode: {}", e),
            }
        }
    }

    #[cfg(all(feature = "embeddings", feature = "rerank"))]
    async fn load_local_reranker(
        &self,
        model: &str,
    ) -> Option<Arc<dyn post_cortex_embeddings::Reranker>> {
        match post_cortex_embeddings::rerank::OnnxCrossEncoder::load(model, RERANKER_MAX_LENGTH)
            .await
        {
            Ok(reranker) => {
                debug!("Reranking search results with {}", model);
                Some(Arc::new(reranker))
            }
            Err(e) => {
                warn!("Failed to load reranker {}: {:#}", model, e);
                None
            }
        }
    }

    #[cfg(all(feature = "embeddings", not(feature = "rerank")))]
    async fn load_local_reranker(
        &self,
        model: &str,
    ) -> Option<Arc<dyn post_cortex_embeddings::Reranker>> {
        warn!(
            "reranker_model = {} is set but this build has no `rerank` feature",
            model
        );
        None
    }

    #[cfg(all(feature = "embeddings", feature = "rerank-voyage"))]
    fn load_voyage_reranker(
        &self,
        key: &str,
        model: &str,
    ) -> Option<Arc<dyn post_cortex_embeddings::Reranker>> {
        match post_cortex_embeddings::rerank::VoyageReranker::new(
            key.to_string(),
            model.to_string(),
            VOYAGE_MAX_DOC_TOKENS,
            VOYAGE_TIMEOUT,
        ) {
            Ok(reranker) => {
                debug!("Reranking search results with Voyage {}", model);
                Some(Arc::new(reranker))
            }
            Err(e) => {
                warn!("Failed to set up the Voyage reranker: {:#}", e);
                None
            }
        }
    }

    #[cfg(all(feature = "embeddings", not(feature = "rerank-voyage")))]
    fn load_voyage_reranker(
        &self,
        _key: &str,
        _model: &str,
    ) -> Option<Arc<dyn post_cortex_embeddings::Reranker>> {
        warn!("A Voyage API key is set but this build has no `rerank-voyage` feature");
        None
    }

    /// Vectorize one update in the background. Fire-and-forget; failures
    /// are logged. Returns immediately **after** the vectorizer has been
    /// initialised — on first call this still waits for the model download
    /// (~50 MB for `potion-multilingual-128M`). Used by direct callers of
    /// `ConversationMemorySystem::add_incremental_update`; the canonical
    /// write path ([`MemoryServiceImpl`](crate::services::MemoryServiceImpl))
    /// vectorizes on the [`crate::pipeline::Pipeline`] instead, so its
    /// callers never see the model-load cost on the hot path.
    #[cfg(feature = "embeddings")]
    pub async fn spawn_background_vectorization(
        &self,
        session_id: Uuid,
        session_arc: Arc<ArcSwap<ActiveSession>>,
        update_id: Uuid,
    ) {
        if !self.config.enable_embeddings || !self.config.auto_vectorize_on_update {
            return;
        }

        let vectorizer = match self.ensure_vectorizer_initialized().await {
            Ok(v) => v,
            Err(e) => {
                debug!("Vectorizer init failed (non-fatal): {}", e);
                return;
            }
        };

        let storage_actor = self.storage_actor.clone();
        tokio::spawn(async move {
            // Owned snapshot: an ArcSwap guard must not live across the
            // embedding await.
            let session = session_arc.load_full();
            match vectorizer.vectorize_update(&session, update_id).await {
                Ok(count) if count > 0 => {
                    if storage_actor.is_deleted(session_id, &session_arc) {
                        // Deleted while embedding: undo, or the vectors outlive it.
                        super::system::undo_vectorization(&vectorizer, session_id, update_id).await;
                        return;
                    }
                    storage_actor.persist_session_nowait(&session_arc, vec![]);
                    debug!(
                        "Background vectorization: {} update(s) for session {}",
                        count, session_id
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    debug!(
                        "Background vectorization failed for session {}: {}",
                        session_id, e
                    );
                }
            }
        });
    }

    /// Load the embedding model and the persisted vectors now instead of on
    /// the first search. A daemon calls this in the background at startup:
    /// the lazy path costs seconds (model + every stored vector), which made
    /// the first query after a restart time out in short-deadline clients.
    #[cfg(feature = "embeddings")]
    pub async fn warm_up_embeddings(&self) -> Result<(), String> {
        if !self.config.enable_embeddings {
            return Ok(());
        }
        self.ensure_vectorizer_initialized().await.map(|_| ())
    }

    /// Lazy-initialize content vectorizer on first use with retry mechanism
    #[cfg(feature = "embeddings")]
    pub(crate) async fn ensure_vectorizer_initialized(
        &self,
    ) -> Result<Arc<ContentVectorizer>, String> {
        // Check if already initialized — fast path, no counter increment
        if let Some(vectorizer) = self.content_vectorizer.get() {
            return Ok(Arc::clone(vectorizer));
        }

        // Note: We don't increment the counter here because concurrent callers
        // would burn through the limit. The counter is incremented only on actual
        // initialization failure inside the closure.
        let attempt = self
            .embedding_config_holder
            .init_attempt_count
            .load(Ordering::Relaxed)
            + 1;

        // Check if we've exceeded max retries (set by actual failures, not callers)
        if attempt > MAX_VECTORIZER_INIT_RETRIES as u64 + 1 {
            if let Some(last_error) = self.embedding_config_holder.last_init_error.read().as_ref() {
                return Err(format!(
                    "Vectorizer initialization failed after {} attempts. Last error: {}",
                    attempt - 1,
                    last_error
                ));
            }
            return Err(format!(
                "Vectorizer initialization failed after {} attempts",
                attempt - 1
            ));
        }

        info!(
            "Lazy-initializing content vectorizer (attempt {}/{})...",
            attempt,
            MAX_VECTORIZER_INIT_RETRIES + 1
        );

        // Try to initialize with retry logic
        let result: Result<&Arc<ContentVectorizer>, String> = self
            .content_vectorizer
            .get_or_try_init(|| async {
                let embedding_config = EmbeddingConfig {
                    model_type: self.embedding_config_holder.model_type,
                    max_batch_size: 32,
                    ..Default::default()
                };

                let vector_db_config = VectorDbConfig {
                    dimension: self.embedding_config_holder.vector_dimension,
                    // `max_vectors` is a hard cap on the whole DB (all
                    // sessions); the deprecated `max_vectors_per_session` is not
                    // that and is ignored, so stay unbounded.
                    max_vectors: usize::MAX,
                    full_text_index: !self.config.storage_has_full_text(),
                    ..Default::default()
                };

                let vectorizer_config = ContentVectorizerConfig {
                    embedding_config,
                    vector_db_config,
                    enable_cross_session_search: self
                        .embedding_config_holder
                        .cross_session_search_enabled,
                    ..Default::default()
                };

                let mut vectorizer = ContentVectorizer::new(vectorizer_config)
                    .await
                    .map_err(|e| format!("Failed to initialize content vectorizer: {}", e))?;

                // Set persistent storage for embedding persistence
                vectorizer.set_persistent_storage(self.vector_storage.clone());

                self.attach_reranker(&mut vectorizer).await;

                // Load persisted embeddings from storage (best-effort, don't fail init)
                match vectorizer.load_all_embeddings_from_storage().await {
                    Ok(count) => {
                        if count > 0 {
                            info!("Loaded {} persisted embeddings from storage during initialization", count);
                        }
                    }
                    Err(e) => {
                        // Don't fail initialization — embeddings will be re-vectorized on demand.
                        // SurrealDB WS can have transient errors under concurrent access.
                        warn!("Failed to load persisted embeddings (non-fatal, will re-vectorize on demand): {}", e);
                    }
                }

                Ok(Arc::new(vectorizer))
            })
            .await;

        match result {
            Ok(vectorizer) => {
                info!(
                    "Content vectorizer initialized successfully on attempt {}",
                    attempt
                );
                // Clear any previous error
                *self.embedding_config_holder.last_init_error.write() = None;

                Ok(Arc::clone(vectorizer))
            }
            Err(e) => {
                // Increment counter only on actual failure (not on concurrent caller contention)
                let real_attempt = self
                    .embedding_config_holder
                    .init_attempt_count
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                // Store the error for diagnostics
                *self.embedding_config_holder.last_init_error.write() = Some(e.clone());
                error!(
                    "Vectorizer initialization failed on attempt {}: {}",
                    real_attempt, e
                );
                Err(e)
            }
        }
    }

    /// Lazy-initialize semantic query engine on first use
    #[cfg(feature = "embeddings")]
    pub async fn ensure_semantic_engine_initialized(
        &self,
    ) -> Result<Arc<crate::semantic_query_engine::SemanticQueryEngine>, String> {
        if let Some(engine) = self.semantic_query_engine.get() {
            return Ok(Arc::clone(engine));
        }

        // Ensure vectorizer is initialized first
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Initialize semantic engine
        self.semantic_query_engine
            .get_or_try_init(|| async {
                info!("Lazy-initializing semantic query engine...");

                use crate::semantic_query_engine::{SemanticQueryConfig, SemanticQueryEngine};

                let config = SemanticQueryConfig {
                    cross_session_enabled: self
                        .embedding_config_holder
                        .cross_session_search_enabled,
                    similarity_threshold: self.config.semantic_search_threshold,
                    ..Default::default()
                };

                let engine = SemanticQueryEngine::new((*vectorizer).clone(), config);

                Ok(Arc::new(engine))
            })
            .await
            .map(Arc::clone)
    }

    /// Vectorize a session's content: its updates and entities, and the
    /// captured messages that have no vectors yet (an embedding that failed
    /// on append). Returns how many vectors were added.
    #[cfg(feature = "embeddings")]
    pub async fn vectorize_session(&self, session_id: Uuid) -> Result<usize, String> {
        let updates = self.vectorize_session_updates(session_id).await?;
        let messages = self.vectorize_session_messages(session_id).await?;
        Ok(updates + messages)
    }

    /// Vectorize a session's updates and entities (requires embeddings
    /// feature). Searches call this when a session lacks update vectors.
    #[cfg(feature = "embeddings")]
    pub(crate) async fn vectorize_session_updates(
        &self,
        session_id: Uuid,
    ) -> Result<usize, String> {
        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Load session (owned snapshot: no ArcSwap guard across the await)
        let session = self.get_session(session_id).await?.load_full();

        // Vectorize content
        match vectorizer.vectorize_session(&session).await {
            Ok(count) => {
                debug!("Vectorized {} items for session {}", count, session_id);
                Ok(count)
            }
            Err(e) => {
                warn!("Failed to vectorize session {session_id}: {e}");
                Err(format!("Vectorization failed: {e}"))
            }
        }
    }

    /// Auto-vectorize only the latest update (incremental vectorization)
    /// This is much more efficient than re-vectorizing the entire session
    /// Includes retry mechanism for transient failures
    #[cfg(feature = "embeddings")]
    pub async fn auto_vectorize_if_enabled(&self, session_id: Uuid) -> Result<(), String> {
        if !self.config.enable_embeddings || !self.config.auto_vectorize_on_update {
            return Ok(());
        }

        // Lazy-initialize vectorizer if needed
        let vectorizer = match self.ensure_vectorizer_initialized().await {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to initialize vectorizer: {}", e);
                return Ok(()); // Don't fail the main operation
            }
        };

        // Load session
        let session_arc = match self.get_session(session_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "Failed to load session {} for vectorization: {}",
                    session_id, e
                );
                return Ok(()); // Don't fail the main operation
            }
        };

        let session = session_arc.load_full();

        // Retry loop for vectorization with exponential backoff
        let mut last_error = None;
        for attempt in 1..=MAX_VECTORIZATION_RETRIES {
            match vectorizer.vectorize_latest_update(&session).await {
                Ok(count) => {
                    debug!(
                        "Incrementally vectorized {} update(s) for session {} (attempt {})",
                        count, session_id, attempt
                    );

                    // The vectorizer invalidates the affected cache entries itself.
                    if count > 0 {
                        // Fire-and-forget persist to save the updated vectorized_update_ids
                        self.storage_actor
                            .persist_session_nowait(&session_arc, vec![]);
                        debug!("Session {} vectorization persist enqueued", session_id);
                    }

                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e.to_string());

                    if attempt < MAX_VECTORIZATION_RETRIES {
                        // Calculate exponential backoff delay
                        let delay_ms = VECTORIZATION_RETRY_DELAY_MS * (1 << (attempt - 1));
                        debug!(
                            "Vectorization attempt {} failed for session {}, retrying in {}ms: {}",
                            attempt, session_id, delay_ms, e
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        // All retries exhausted
        if let Some(error) = last_error {
            warn!(
                "Incremental vectorization failed for session {} after {} retries: {}",
                session_id, MAX_VECTORIZATION_RETRIES, error
            );
        }

        Ok(()) // Don't fail the main operation even after retries exhausted
    }

    /// Vectorize all sessions in the system with parallel processing
    /// Returns total number of vectorized items across all sessions and statistics
    /// Uses a semaphore to limit concurrent vectorization tasks
    #[cfg(feature = "embeddings")]
    pub async fn vectorize_all_sessions(&self) -> Result<(usize, usize, usize), String> {
        info!("Starting full vectorization of all sessions (parallel mode)");
        let start_time = std::time::Instant::now();

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Get all session IDs
        let session_ids = self.list_sessions().await?;
        let total_sessions = session_ids.len();

        if total_sessions == 0 {
            info!("No sessions found to vectorize");
            return Ok((0, 0, 0));
        }

        info!(
            "Found {} sessions to vectorize (max {} parallel tasks)",
            total_sessions, MAX_PARALLEL_VECTORIZATION
        );

        // Shared counters for parallel processing
        let total_vectorized = Arc::new(AtomicUsize::new(0));
        let successful_sessions = Arc::new(AtomicUsize::new(0));
        let failed_sessions = Arc::new(AtomicUsize::new(0));
        let processed_count = Arc::new(AtomicUsize::new(0));

        // Semaphore to limit concurrency
        let semaphore = Arc::new(Semaphore::new(MAX_PARALLEL_VECTORIZATION));

        // Process sessions in parallel with limited concurrency
        let mut handles = Vec::with_capacity(total_sessions);

        for session_id in session_ids {
            let vectorizer = Arc::clone(&vectorizer);
            let semaphore = Arc::clone(&semaphore);
            let total_vectorized = Arc::clone(&total_vectorized);
            let successful_sessions = Arc::clone(&successful_sessions);
            let failed_sessions = Arc::clone(&failed_sessions);
            let processed_count = Arc::clone(&processed_count);

            // Owned snapshot of the session for the spawned task
            let session_data = match self.get_session(session_id).await {
                Ok(arc) => Some(arc.load_full()),
                Err(e) => {
                    failed_sessions.fetch_add(1, Ordering::Relaxed);
                    let count = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
                    warn!(
                        "[{}/{}] Failed to load session {}: {}",
                        count, total_sessions, session_id, e
                    );
                    None
                }
            };

            if let Some(session) = session_data {
                let handle = tokio::spawn(async move {
                    // Acquire semaphore permit
                    let _permit = semaphore.acquire().await.expect("Semaphore closed");

                    // Check if session already has embeddings
                    let already_vectorized = vectorizer.is_session_vectorized(session_id);
                    if already_vectorized {
                        let existing_count = vectorizer.count_session_embeddings(session_id);
                        debug!(
                            "Session {} already has {} embeddings, re-vectorizing...",
                            session_id, existing_count
                        );
                    }

                    // Vectorize the session
                    match vectorizer.vectorize_session(&session).await {
                        Ok(count) => {
                            total_vectorized.fetch_add(count, Ordering::Relaxed);
                            successful_sessions.fetch_add(1, Ordering::Relaxed);
                            let processed = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
                            info!(
                                "[{}/{}] Vectorized {} items for session {}",
                                processed, total_sessions, count, session_id
                            );
                        }
                        Err(e) => {
                            failed_sessions.fetch_add(1, Ordering::Relaxed);
                            let processed = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
                            warn!(
                                "[{}/{}] Failed to vectorize session {}: {}",
                                processed, total_sessions, session_id, e
                            );
                        }
                    }
                });

                handles.push(handle);
            }
        }

        // Wait for all tasks to complete
        for handle in handles {
            let _ = handle.await;
        }

        // Clear query cache after bulk vectorization
        if let Err(e) = vectorizer.clear_query_cache().await {
            warn!(
                "Failed to clear query cache after bulk vectorization: {}",
                e
            );
        }

        let elapsed = start_time.elapsed();
        let total = total_vectorized.load(Ordering::Relaxed);
        let success = successful_sessions.load(Ordering::Relaxed);
        let failed = failed_sessions.load(Ordering::Relaxed);

        info!(
            "Bulk vectorization complete in {:.2}s: {} total items across {} successful sessions ({} failed)",
            elapsed.as_secs_f64(),
            total,
            success,
            failed
        );

        Ok((total, success, failed))
    }

    /// Semantic search across all sessions. `options.limit` defaults to
    /// [`DEFAULT_SEARCH_LIMIT`].
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_global(
        &self,
        query: &str,
        mut options: crate::content_vectorizer::SearchOptions,
    ) -> Result<Vec<crate::content_vectorizer::SemanticSearchResult>, String> {
        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        let limit = *options.limit.get_or_insert(DEFAULT_SEARCH_LIMIT);
        match vectorizer
            .semantic_search(query, limit, None, options)
            .await
        {
            Ok(results) => Ok(self.complete_results(results).await),
            Err(e) => Err(format!("Semantic search failed: {e}")),
        }
    }

    /// Semantic search within a specific session. `options.limit` defaults
    /// to [`DEFAULT_SEARCH_LIMIT`].
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_session(
        &self,
        session_id: Uuid,
        query: &str,
        mut options: crate::content_vectorizer::SearchOptions,
    ) -> Result<Vec<crate::content_vectorizer::SemanticSearchResult>, String> {
        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Auto-load session if not already loaded (get_session_internal handles this)
        // This ensures the session is in memory before vectorization
        // We keep a reference to use for Graph-RAG enrichment
        let session_arc = self.get_session(session_id).await?;

        // Auto-vectorize if the session has content not vectorized yet
        if vectorizer.needs_vectorization(&session_arc.load_full()) {
            debug!(
                "Session {} not vectorized, auto-vectorizing before search",
                session_id
            );
            if let Err(e) = self.vectorize_session_updates(session_id).await {
                warn!(
                    "Auto-vectorization failed for session {}: {}",
                    session_id, e
                );
                // Continue anyway - search might still work with partial data
            }
        }

        if vectorizer.reranks(options.mode) {
            let session = session_arc.load();
            options.graph_candidates = Some(self.graph_candidates(&vectorizer, &session, query));
        }
        let limit = *options.limit.get_or_insert(DEFAULT_SEARCH_LIMIT);
        match vectorizer
            .semantic_search(query, limit, Some(session_id), options)
            .await
        {
            Ok(results) => {
                let session = session_arc.load_full();
                let results = enrich_results_with_graph(&session, query, results);
                let sessions = std::collections::HashMap::from([(session_id, session)]);
                let results = super::neighbours::complete_results(
                    results,
                    &sessions,
                    self.config.neighbour_radius,
                );
                Ok(self.complete_message_results(results).await)
            }
            Err(e) => Err(format!("Session semantic search failed: {e}")),
        }
    }

    /// Content ids the session's retrieval graph ranks for `query`, best
    /// first. The graph joins the session's records through the terms and
    /// agent entities stored with their vectors and the entity relations; it
    /// is cached per session and rebuilt when the session's vector or
    /// relation count changes.
    #[cfg(feature = "embeddings")]
    fn graph_candidates(
        &self,
        vectorizer: &crate::content_vectorizer::ContentVectorizer,
        session: &ActiveSession,
        query: &str,
    ) -> Arc<[String]> {
        let session_id = session.id();
        let vectors = vectorizer.count_session_embeddings(session_id);
        let relations = session.entity_graph.edge_count();
        let cached = self
            .retrieval_graphs
            .get(&session_id)
            .filter(|entry| entry.0 == vectors && entry.1 == relations)
            .map(|entry| Arc::clone(&entry.2));
        let graph = cached.unwrap_or_else(|| {
            let graph = Arc::new(RetrievalGraph::build(
                vectorizer.graph_records(session_id),
                &session.entity_graph.get_all_relationships(),
            ));
            self.retrieval_graphs
                .insert(session_id, (vectors, relations, Arc::clone(&graph)));
            graph
        });
        graph
            .rank(query, crate::content_vectorizer::RERANK_POOL)
            .into()
    }

    /// Complete results from their sessions: full text for long update and
    /// message hits and the neighbours of every update and message hit
    /// (`SystemConfig::neighbour_radius`). Sessions that fail to load keep
    /// their results as they are.
    #[cfg(feature = "embeddings")]
    async fn complete_results(
        &self,
        results: Vec<crate::content_vectorizer::SemanticSearchResult>,
    ) -> Vec<crate::content_vectorizer::SemanticSearchResult> {
        if results.is_empty() {
            return results;
        }
        let ids: std::collections::HashSet<Uuid> = results.iter().map(|r| r.session_id).collect();
        let mut sessions = std::collections::HashMap::with_capacity(ids.len());
        for id in ids {
            match self.get_session(id).await {
                Ok(session) => {
                    sessions.insert(id, session.load_full());
                }
                Err(e) => warn!("Results of session {} left incomplete: {}", id, e),
            }
        }
        let results =
            super::neighbours::complete_results(results, &sessions, self.config.neighbour_radius);
        self.complete_message_results(results).await
    }

    /// Find related content across sessions
    #[cfg(feature = "embeddings")]
    pub async fn find_related_content(
        &self,
        session_id: Uuid,
        topic: &str,
        limit: Option<usize>,
    ) -> Result<Vec<crate::content_vectorizer::SemanticSearchResult>, String> {
        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Auto-load session if not already loaded (owned snapshot: it is
        // used across awaits below)
        let session = self.get_session(session_id).await?.load_full();

        // Auto-vectorize if the session has content not vectorized yet
        if vectorizer.needs_vectorization(&session) {
            debug!(
                "Session {} not vectorized, auto-vectorizing before related content search",
                session_id
            );
            if let Err(e) = self.vectorize_session_updates(session_id).await {
                warn!(
                    "Auto-vectorization failed for session {}: {}",
                    session_id, e
                );
                // Continue anyway - search might still work with partial data
            }
        }

        match vectorizer
            .find_related_content(&session, topic, limit.unwrap_or(10))
            .await
        {
            Ok(results) => Ok(results),
            Err(e) => Err(format!("Related content search failed: {e}")),
        }
    }

    /// Semantic search across `session_ids`. `options.limit` defaults to
    /// [`DEFAULT_SEARCH_LIMIT`].
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_multisession(
        &self,
        session_ids: &[Uuid],
        query: &str,
        mut options: crate::content_vectorizer::SearchOptions,
    ) -> Result<Vec<crate::content_vectorizer::SemanticSearchResult>, String> {
        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        let limit = *options.limit.get_or_insert(DEFAULT_SEARCH_LIMIT);
        match vectorizer
            .semantic_search_multisession(query, limit, session_ids, options)
            .await
        {
            Ok(results) => Ok(self.complete_results(results).await),
            Err(e) => Err(format!("Multisession semantic search failed: {e}")),
        }
    }

    /// Get vectorization statistics
    #[cfg(feature = "embeddings")]
    pub fn get_vectorization_stats(
        &self,
    ) -> Result<std::collections::HashMap<String, usize>, String> {
        // Check if vectorizer has been initialized
        if let Some(vectorizer) = self.content_vectorizer.get() {
            Ok(vectorizer.get_vectorization_stats())
        } else {
            Err("Embeddings not initialized yet (call any vectorization method first)".to_string())
        }
    }

    /// Check if embeddings are enabled and initialized
    pub fn embeddings_enabled(&self) -> bool {
        self.config.enable_embeddings && cfg!(feature = "embeddings") && {
            #[cfg(feature = "embeddings")]
            {
                self.content_vectorizer.get().is_some()
            }
            #[cfg(not(feature = "embeddings"))]
            {
                false
            }
        }
    }

    /// Enable embeddings at runtime (requires restart to initialize components)
    pub async fn enable_embeddings_config(&mut self) -> Result<(), String> {
        if !cfg!(feature = "embeddings") {
            return Err("Embeddings feature not compiled in".to_string());
        }

        self.config.enable_embeddings = true;
        Ok(())
    }

    /// Configure embedding model type
    pub async fn set_embedding_model(&mut self, model_type: String) -> Result<(), String> {
        self.config.embeddings_model_type = model_type;
        Ok(())
    }

    /// Invalidate a source file and rebuild the entity graph for the given session.
    ///
    /// 1. Removes SourceReference entries for the file (storage layer)
    /// 2. Removes incremental updates referencing the file from the session
    /// 3. Rebuilds entity graph from remaining updates
    /// 4. Persists the updated session
    ///
    /// Returns (entries_invalidated, entities_after_rebuild).
    pub async fn invalidate_and_rebuild_entity_graph(
        &self,
        session_id: Uuid,
        file_path: &str,
    ) -> Result<(u32, usize), String> {
        // Step 1: invalidate source references in storage
        let entries_invalidated = self.storage_actor.invalidate_source(file_path).await?;

        // Step 2-4: update session (remove stale updates, rebuild graph, persist)
        let session_arc = self
            .session_manager
            .get_or_create_session(session_id)
            .await?;

        let current = session_arc.load_full();
        let mut new_session = (*current).clone();

        let removed = new_session.remove_updates_for_file(file_path);
        if removed > 0 {
            let remaining: std::collections::HashSet<Uuid> = new_session
                .incremental_updates
                .iter()
                .map(|u| u.id)
                .collect();
            let removed_ids: Vec<String> = current
                .incremental_updates
                .iter()
                .filter(|u| !remaining.contains(&u.id))
                .map(|u| u.id.to_string())
                .collect();
            match new_session.rebuild_entity_graph_from_updates().await {
                Ok((before, after)) => {
                    info!(
                        "Invalidate+rebuild for {}: {} source refs, {} updates removed, entities {} -> {}",
                        file_path, entries_invalidated, removed, before, after,
                    );
                    let entities_after = after;
                    let new_arc = Arc::new(new_session);
                    let prev = session_arc.compare_and_swap(&current, Arc::clone(&new_arc));
                    if Arc::ptr_eq(&prev, &current) {
                        let removed_uuids: Vec<Uuid> = removed_ids
                            .iter()
                            .filter_map(|id| Uuid::parse_str(id).ok())
                            .collect();
                        for id in &removed_uuids {
                            new_arc.vectorized_update_ids.remove(id);
                        }
                        // Named: storage never infers a removal.
                        self.storage_actor
                            .persist_session_removals_nowait(&session_arc, removed_uuids);
                        self.drop_content_vectors(session_id, &removed_ids).await;
                    } else {
                        warn!(
                            "CAS failed during invalidate+rebuild for session {}",
                            session_id
                        );
                    }
                    Ok((entries_invalidated, entities_after))
                }
                Err(e) => {
                    warn!("Entity graph rebuild failed after invalidation: {}", e);
                    Ok((entries_invalidated, 0))
                }
            }
        } else {
            debug!(
                "No updates reference file {}, skipping entity graph rebuild",
                file_path
            );
            Ok((entries_invalidated, new_session.entity_graph.entity_count()))
        }
    }

    /// Remove the embeddings of deleted content (update ids or
    /// `entity:<name>`) of one session: from the in-memory vector DB, from
    /// persistent storage, and from every cached query they could appear
    /// in. Best-effort: failures are logged, never returned, because the
    /// content itself is already gone.
    pub(crate) async fn drop_content_vectors(&self, session_id: Uuid, content_ids: &[String]) {
        if content_ids.is_empty() {
            return;
        }

        #[cfg(feature = "embeddings")]
        if let Some(vectorizer) = self.content_vectorizer.get() {
            for content_id in content_ids {
                if let Err(e) = vectorizer.remove_content(session_id, content_id).await {
                    warn!(
                        "Failed to remove embedding {} of session {}: {:#}",
                        content_id, session_id, e
                    );
                }
            }
            return;
        }

        // Vectorizer not initialised: nothing is in memory or cached yet,
        // but the persisted copy would be loaded at init.
        for content_id in content_ids {
            // Session-scoped: entity content ids repeat across sessions.
            if let Err(e) = self
                .vector_storage
                .remove_session_vector(&session_id.to_string(), content_id)
                .await
            {
                warn!(
                    "Failed to remove persisted embedding {} of session {}: {:#}",
                    content_id, session_id, e
                );
            }
        }
    }

    /// Clear query cache to prevent stale vector IDs after restart
    ///
    /// This should be called on daemon startup to ensure cached query results
    /// don't reference vector IDs from before the restart, which would cause
    /// incorrect similarity calculations.
    pub async fn clear_query_cache(&self) -> Result<(), String> {
        #[cfg(feature = "embeddings")]
        {
            if let Some(vectorizer) = self.content_vectorizer.get() {
                vectorizer
                    .clear_query_cache()
                    .await
                    .map_err(|e| format!("Failed to clear query cache: {}", e))?;
                info!("Query cache cleared successfully");
            }
        }
        Ok(())
    }
}

/// Extract candidate entity names from text: lowercase words >3 chars, stripped of
/// non-alphanumeric edge characters (preserving `_` and `-`).
#[cfg(feature = "embeddings")]
fn extract_entities(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|w| {
            let clean = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
            (clean.len() > 3).then(|| clean.to_lowercase())
        })
        .collect()
}

/// Graph-RAG enrichment of semantic search results using the session's entity graph.
///
/// Steps:
/// 1. Map query entities → their graph neighbors (global insights).
/// 2. For each result, enrich text with local entity neighborhoods.
/// 3. If the top two results contain distinct entities, surface any shortest path
///    between them as a "Structural Insight".
/// 4. Prepend the synthesized insights to the first result.
#[cfg(feature = "embeddings")]
fn enrich_results_with_graph(
    session: &post_cortex_core::session::active_session::ActiveSession,
    query: &str,
    results: Vec<crate::content_vectorizer::SemanticSearchResult>,
) -> Vec<crate::content_vectorizer::SemanticSearchResult> {
    let entity_graph = &session.entity_graph;

    tracing::debug!(
        "Graph-RAG: enrichment for {} results (graph has {} entities)",
        results.len(),
        entity_graph.entity_count()
    );

    // Step 1: query → neighbors map
    let query_entities = extract_entities(query);
    let mut global_graph_map: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for q_entity in &query_entities {
        let relations = entity_graph.find_related_entities(q_entity);
        if !relations.is_empty() {
            global_graph_map.insert(q_entity.clone(), relations);
        }
    }

    let mut graph_insights = String::new();
    if !global_graph_map.is_empty() {
        graph_insights.push_str("\n[System Knowledge Map]:\n");
        for (entity, rels) in &global_graph_map {
            graph_insights.push_str(&format!(
                "- {} is central to: {}\n",
                entity,
                rels.join(", ")
            ));
        }
    }

    // Step 2: per-result local enrichment
    let mut enriched: Vec<_> = results
        .into_iter()
        .map(|mut result| {
            let mut chunk_entities = extract_entities(&result.text_content);
            chunk_entities.sort();
            chunk_entities.dedup();

            let mut local_rels = Vec::new();
            for entity in chunk_entities.iter().take(2) {
                if global_graph_map.contains_key(entity) {
                    continue;
                }
                let relations = entity_graph.find_related_entities(entity);
                if !relations.is_empty() {
                    let limited: Vec<_> = relations.iter().take(5).cloned().collect();
                    local_rels.push(format!("{}: {}", entity, limited.join(", ")));
                }
            }
            if !local_rels.is_empty() {
                result.text_content = format!(
                    "{}\n(Graph expansion: {})",
                    result.text_content,
                    local_rels.join(" | ")
                );
            }
            result
        })
        .collect();

    // Step 3: shortest-path insight between top two results
    if enriched.len() >= 2 {
        let top1 = extract_entities(&enriched[0].text_content);
        let top2 = extract_entities(&enriched[1].text_content);
        if let (Some(e1), Some(e2)) = (top1.first(), top2.first())
            && e1 != e2
            && let Some(path) = entity_graph.find_shortest_path(e1, e2)
            && path.len() > 2
        {
            graph_insights.push_str(&format!(
                "\n[Structural Insight]: Found connection: {}\n",
                path.join(" -> ")
            ));
        }
    }

    // Step 4: prepend insights to first result
    if !graph_insights.is_empty() && !enriched.is_empty() {
        enriched[0].text_content = format!("{}{}\n---\n", graph_insights, enriched[0].text_content);
    }

    enriched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_system::SystemConfig;
    use post_cortex_core::core::context_update::EntityType;
    use post_cortex_embeddings::VectorMetadata;

    async fn system(suffix: &str) -> (ConversationMemorySystem, tempfile::TempDir) {
        let dir = tempfile::Builder::new()
            .prefix(&format!("pcx_mems_{suffix}_"))
            .tempdir()
            .unwrap();
        let config = SystemConfig {
            data_directory: dir.path().to_string_lossy().into_owned(),
            // No model load: these tests cover the paths that must work
            // before (or without) the vectorizer.
            enable_embeddings: false,
            ..Default::default()
        };
        (ConversationMemorySystem::new(config).await.unwrap(), dir)
    }

    /// Persist an embedding for `content_id`, in whatever dimension the
    /// storage backend accepts.
    async fn persist_embedding(sys: &ConversationMemorySystem, session_id: Uuid, content_id: &str) {
        for dim in [sys.config.vector_dimension, 384, 256] {
            let metadata = VectorMetadata::new(
                content_id.to_string(),
                "text".to_string(),
                session_id.to_string(),
                "UpdateContent".to_string(),
            );
            if sys
                .vector_storage
                .add_vector(vec![0.5; dim], metadata)
                .await
                .is_ok()
            {
                return;
            }
        }
        panic!("storage accepted no embedding dimension");
    }

    async fn persisted_count(sys: &ConversationMemorySystem, session_id: Uuid) -> usize {
        sys.vector_storage
            .count_session_embeddings(&session_id.to_string())
            .await
    }

    /// Poll the stored session until `check` holds (persists are queued).
    async fn wait_for_stored(
        sys: &ConversationMemorySystem,
        session_id: Uuid,
        check: impl Fn(&ActiveSession) -> bool,
    ) -> bool {
        for _ in 0..100 {
            if let Ok(Some(stored)) = sys.storage_actor.load_session(session_id).await
                && check(&stored)
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn delete_update_removes_persisted_embedding() {
        let (sys, _dir) = system("delupd").await;
        let sid = sys.create_session(None, None).await.unwrap();
        sys.add_incremental_update(sid, "an update worth keeping".into(), None)
            .await
            .unwrap();
        let entry_id = sys
            .get_session(sid)
            .await
            .unwrap()
            .load()
            .incremental_updates[0]
            .id;
        persist_embedding(&sys, sid, &entry_id.to_string()).await;
        assert_eq!(persisted_count(&sys, sid).await, 1);

        assert!(sys.delete_update(sid, entry_id).await.unwrap());
        assert!(!sys.delete_update(sid, entry_id).await.unwrap());

        assert_eq!(
            persisted_count(&sys, sid).await,
            0,
            "deleted update's embedding would be reloaded on restart"
        );
        let session = sys.get_session(sid).await.unwrap().load_full();
        assert!(session.incremental_updates.is_empty());
    }

    #[tokio::test]
    async fn delete_entity_removes_persisted_embedding_and_persists_session() {
        let (sys, _dir) = system("delent").await;
        let sid = sys.create_session(None, None).await.unwrap();
        {
            let arc = sys.get_session(sid).await.unwrap();
            let mut session = (**arc.load()).clone();
            Arc::make_mut(&mut session.entity_graph).add_or_update_entity(
                "Doomed".into(),
                EntityType::Concept,
                chrono::Utc::now(),
                "",
            );
            arc.store(Arc::new(session));
        }
        persist_embedding(&sys, sid, "entity:Doomed").await;

        assert!(sys.delete_entity(sid, "Doomed").await.unwrap());
        assert!(!sys.delete_entity(sid, "Doomed").await.unwrap());
        assert_eq!(persisted_count(&sys, sid).await, 0);

        let persisted = wait_for_stored(&sys, sid, |s| !s.entity_graph.has_entity("Doomed")).await;
        assert!(persisted, "entity removal was not persisted");
    }

    #[tokio::test]
    async fn delete_entity_keeps_other_sessions_embedding_of_the_same_name() {
        let (sys, _dir) = system("delent_scope").await;
        let a = sys.create_session(None, None).await.unwrap();
        let b = sys.create_session(None, None).await.unwrap();
        for sid in [a, b] {
            let arc = sys.get_session(sid).await.unwrap();
            let mut session = (**arc.load()).clone();
            Arc::make_mut(&mut session.entity_graph).add_or_update_entity(
                "Rust".into(),
                EntityType::Technology,
                chrono::Utc::now(),
                "",
            );
            arc.store(Arc::new(session));
            persist_embedding(&sys, sid, "entity:Rust").await;
        }

        assert!(sys.delete_entity(a, "Rust").await.unwrap());
        assert_eq!(persisted_count(&sys, a).await, 0);
        assert_eq!(
            persisted_count(&sys, b).await,
            1,
            "another session's entity:Rust must survive"
        );
    }

    #[tokio::test]
    async fn update_session_metadata_persists() {
        let (sys, _dir) = system("meta").await;
        let sid = sys.create_session(None, None).await.unwrap();

        sys.update_session_metadata(sid, Some("renamed".into()), Some("desc".into()))
            .await
            .unwrap();
        let session = sys.get_session(sid).await.unwrap().load_full();
        assert_eq!(session.name().as_deref(), Some("renamed"));

        let persisted = wait_for_stored(&sys, sid, |s| {
            s.name().as_deref() == Some("renamed") && s.description().as_deref() == Some("desc")
        })
        .await;
        assert!(persisted, "metadata change was not persisted");
    }
}
