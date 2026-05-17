use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::session::active_session::ActiveSession;

#[cfg(feature = "embeddings")]
use crate::core::content_vectorizer::{ContentVectorizer, ContentVectorizerConfig};
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

impl ConversationMemorySystem {
    /// Vectorize the latest update in the background. Fire-and-forget; failures are logged.
    /// Returns immediately after spawning the task.
    #[cfg(feature = "embeddings")]
    pub async fn spawn_background_vectorization(
        &self,
        session_id: Uuid,
        session_arc: Arc<ArcSwap<ActiveSession>>,
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
            let session = session_arc.load();
            match vectorizer.vectorize_latest_update(&session).await {
                Ok(count) if count > 0 => {
                    let _ = vectorizer.invalidate_session_cache(session_id).await;
                    storage_actor
                        .persist_session_and_update_nowait((**session).clone(), vec![]);
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

    /// Lazy-initialize content vectorizer on first use with retry mechanism
    #[cfg(feature = "embeddings")]
    async fn ensure_vectorizer_initialized(&self) -> Result<Arc<ContentVectorizer>, String> {
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
                    max_vectors: self.embedding_config_holder.max_vectors_per_session,
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
    ) -> Result<Arc<crate::core::semantic_query_engine::SemanticQueryEngine>, String> {
        if let Some(engine) = self.semantic_query_engine.get() {
            return Ok(Arc::clone(engine));
        }

        // Ensure vectorizer is initialized first
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Initialize semantic engine
        self.semantic_query_engine
            .get_or_try_init(|| async {
                info!("Lazy-initializing semantic query engine...");

                use crate::core::semantic_query_engine::{
                    SemanticQueryConfig, SemanticQueryEngine,
                };

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

    /// Vectorize a session's content (requires embeddings feature)
    #[cfg(feature = "embeddings")]
    pub async fn vectorize_session(&self, session_id: Uuid) -> Result<usize, String> {
        let _timer = self.performance_monitor.start_timer("vectorize_session");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Load session
        let session_result = self.get_session(session_id).await?;
        let session = session_result.load();

        // Vectorize content
        match vectorizer.vectorize_session(&session).await {
            Ok(count) => {
                info!("Vectorized {} items for session {}", count, session_id);
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

        let session = session_arc.load();

        // Retry loop for vectorization with exponential backoff
        let mut last_error = None;
        for attempt in 1..=MAX_VECTORIZATION_RETRIES {
            match vectorizer.vectorize_latest_update(&session).await {
                Ok(count) => {
                    info!(
                        "Incrementally vectorized {} update(s) for session {} (attempt {})",
                        count, session_id, attempt
                    );

                    // Invalidate only this session's cache entries instead of clearing all
                    // This is handled by the vectorizer internally now
                    if count > 0 {
                        if let Err(e) = vectorizer.invalidate_session_cache(session_id).await {
                            debug!(
                                "Cache invalidation for session {} (non-critical): {}",
                                session_id, e
                            );
                        }

                        // Fire-and-forget persist to save the updated vectorized_update_ids
                        self.storage_actor.persist_session_and_update_nowait(
                            (**session).clone(),
                            vec![],
                        );
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

            // Clone session_arc data we need before spawning
            let session_data = match self.get_session(session_id).await {
                Ok(arc) => Some(arc.load().as_ref().clone()),
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

    /// Perform semantic search across all sessions
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_global(
        &self,
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("semantic_search_global");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(limit.unwrap_or(20)),
            date_range,
            recency_bias,
        };

        match vectorizer
            .semantic_search(query, limit.unwrap_or(20), None, options)
            .await
        {
            Ok(results) => Ok(results),
            Err(e) => Err(format!("Semantic search failed: {e}")),
        }
    }

    /// Perform semantic search within a specific session
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_session(
        &self,
        session_id: Uuid,
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("semantic_search_session");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Auto-load session if not already loaded (get_session_internal handles this)
        // This ensures the session is in memory before vectorization
        // We keep a reference to use for Graph-RAG enrichment
        let session_arc = self.get_session(session_id).await?;

        // Auto-vectorize if session hasn't been vectorized yet
        if !vectorizer.is_session_vectorized(session_id) {
            info!(
                "Session {} not vectorized, auto-vectorizing before search",
                session_id
            );
            if let Err(e) = self.vectorize_session(session_id).await {
                warn!(
                    "Auto-vectorization failed for session {}: {}",
                    session_id, e
                );
                // Continue anyway - search might still work with partial data
            }
        }

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(limit.unwrap_or(20)),
            date_range,
            recency_bias,
        };

        match vectorizer
            .semantic_search(query, limit.unwrap_or(20), Some(session_id), options)
            .await
        {
            Ok(results) => {
                let session = session_arc.load();
                Ok(enrich_results_with_graph(&session, query, results))
            }
            Err(e) => Err(format!("Session semantic search failed: {e}")),
        }
    }

    /// Find related content across sessions
    #[cfg(feature = "embeddings")]
    pub async fn find_related_content(
        &self,
        session_id: Uuid,
        topic: &str,
        limit: Option<usize>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self.performance_monitor.start_timer("find_related_content");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        // Auto-load session if not already loaded
        let session_result = self.get_session(session_id).await?;
        let session = session_result.load();

        // Auto-vectorize if session hasn't been vectorized yet
        if !vectorizer.is_session_vectorized(session_id) {
            info!(
                "Session {} not vectorized, auto-vectorizing before related content search",
                session_id
            );
            if let Err(e) = self.vectorize_session(session_id).await {
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

    /// Perform semantic search across multiple sessions
    #[cfg(feature = "embeddings")]
    pub async fn semantic_search_multisession(
        &self,
        session_ids: &[Uuid],
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<crate::core::content_vectorizer::SemanticSearchResult>, String> {
        let _timer = self
            .performance_monitor
            .start_timer("semantic_search_multisession");

        // Lazy-initialize vectorizer if needed
        let vectorizer = self.ensure_vectorizer_initialized().await?;

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(limit.unwrap_or(20)),
            date_range,
            recency_bias,
        };

        match vectorizer
            .semantic_search_multisession(query, limit.unwrap_or(20), session_ids, options)
            .await
        {
            Ok(results) => Ok(results),
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

        let current = session_arc.load();
        let mut new_session = (**current).clone();

        let removed = new_session.remove_updates_for_file(file_path);
        if removed > 0 {
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
                        self.storage_actor.persist_session_and_update_nowait(
                            (*new_arc).clone(),
                            vec![],
                        );
                    } else {
                        warn!("CAS failed during invalidate+rebuild for session {}", session_id);
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
    session: &crate::session::active_session::ActiveSession,
    query: &str,
    results: Vec<crate::core::content_vectorizer::SemanticSearchResult>,
) -> Vec<crate::core::content_vectorizer::SemanticSearchResult> {
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
        if let (Some(e1), Some(e2)) = (top1.first(), top2.first()) {
            if e1 != e2 {
                if let Some(path) = entity_graph.find_shortest_path(e1, e2) {
                    if path.len() > 2 {
                        graph_insights.push_str(&format!(
                            "\n[Structural Insight]: Found connection: {}\n",
                            path.join(" -> ")
                        ));
                    }
                }
            }
        }
    }

    // Step 4: prepend insights to first result
    if !graph_insights.is_empty() && !enriched.is_empty() {
        enriched[0].text_content =
            format!("{}{}\n---\n", graph_insights, enriched[0].text_content);
    }

    enriched
}
