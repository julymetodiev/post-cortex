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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//! Content Vectorization Module
//!
//! This module handles the conversion of text content into vector representations
//! for semantic search and similarity analysis.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::core::context_update::ContextUpdate;
use crate::core::embeddings::{EmbeddingConfig, LocalEmbeddingEngine};
use crate::core::query_cache::{QueryCache, QueryCacheConfig};
use crate::core::vector_db::{VectorDB, VectorDbConfig, VectorMetadata};
use crate::session::active_session::ActiveSession;

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

/// Threshold for switching to parallel processing
/// Below this threshold, sequential processing is faster due to reduced overhead
const PARALLEL_PROCESSING_THRESHOLD: usize = 50;

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

/// Search result with enhanced metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchResult {
    pub content_id: String,
    pub session_id: Uuid,
    pub content_type: ContentType,
    pub text_content: String,
    pub similarity_score: f32,
    pub importance_score: f32,
    pub timestamp: DateTime<Utc>,
    pub combined_score: f32,
}

/// Configuration for content vectorization
#[derive(Debug, Clone)]
pub struct ContentVectorizerConfig {
    pub embedding_config: EmbeddingConfig,
    pub vector_db_config: VectorDbConfig,
    pub min_text_length: usize,
    pub max_text_length: usize,
    pub batch_size: usize,
    pub enable_entity_vectorization: bool,
    pub enable_cross_session_search: bool,
    pub query_cache_config: QueryCacheConfig,
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
            max_text_length: 2000,
            batch_size: 32,
            enable_entity_vectorization: true,
            enable_cross_session_search: true,
            query_cache_config: QueryCacheConfig::default(),
            enable_query_caching: true,
            recency_bias: 0.0, // Disabled by default for backward compatibility
        }
    }
}

use std::sync::Arc;

/// Search options for semantic queries
///
/// This struct consolidates all optional parameters for semantic search,
/// eliminating the need for multiple method variants (e.g., _with_recency).
///
/// # Examples
///
/// ```
/// // Search with recency bias
/// let options = SearchOptions {
///     limit: Some(10),
///     recency_bias: Some(0.5),
///     ..Default::default()
/// };
///
/// // Search with date range
/// let options = SearchOptions {
///     date_range: Some((start, end)),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Maximum number of results to return (None = use default)
    pub limit: Option<usize>,

    /// Optional date range filter (start, end)
    pub date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,

    /// Recency bias parameter (0.0 = disabled, higher = more recent content preferred)
    pub recency_bias: Option<f32>,
}

/// Main content vectorization pipeline
pub struct ContentVectorizer {
    embedding_engine: Arc<LocalEmbeddingEngine>,
    vector_db: Arc<VectorDB>,
    config: ContentVectorizerConfig,
    query_cache: Option<Arc<QueryCache>>,
    /// Optional persistent storage for embeddings (RocksDB/SurrealDB)
    persistent_storage: Option<Arc<dyn crate::storage::traits::VectorStorage>>,
    /// Recency bias performance metrics - all atomic for lock-free access
    recency_bias_total_duration_ns: Arc<std::sync::atomic::AtomicU64>,
    recency_bias_total_results: Arc<std::sync::atomic::AtomicU64>,
    recency_bias_calculation_count: Arc<std::sync::atomic::AtomicU64>,
}

impl Clone for ContentVectorizer {
    fn clone(&self) -> Self {
        Self {
            embedding_engine: Arc::clone(&self.embedding_engine),
            vector_db: Arc::clone(&self.vector_db),
            config: self.config.clone(),
            query_cache: self.query_cache.clone(),
            persistent_storage: self.persistent_storage.clone(),
            // Note: Atomics are shared between clones via Arc for metrics aggregation
            recency_bias_total_duration_ns: Arc::clone(&self.recency_bias_total_duration_ns),
            recency_bias_total_results: Arc::clone(&self.recency_bias_total_results),
            recency_bias_calculation_count: Arc::clone(&self.recency_bias_calculation_count),
        }
    }
}

impl ContentVectorizer {
    /// Create a new `ContentVectorizer` with the given configuration
    ///
    /// # Errors
    ///
    /// Returns an error if the vector database or content vectorizer fails to initialize
    pub async fn new(config: ContentVectorizerConfig) -> Result<Self> {
        info!(
            "Initializing ContentVectorizer with caching: {}",
            config.enable_query_caching
        );

        let embedding_engine = LocalEmbeddingEngine::new(config.embedding_config.clone())
            .await
            .context("Failed to initialize embedding engine")?;

        let vector_db = VectorDB::new(config.vector_db_config.clone())
            .context("Failed to initialize vector database")?;

        let query_cache = if config.enable_query_caching {
            Some(QueryCache::new(config.query_cache_config.clone()))
        } else {
            None
        };

        debug!(
            "ContentVectorizer initialized successfully with query caching: {}",
            config.enable_query_caching
        );

        Ok(Self {
            embedding_engine: Arc::new(embedding_engine),
            vector_db: Arc::new(vector_db),
            config,
            query_cache: query_cache.map(Arc::new),
            persistent_storage: None,
            recency_bias_total_duration_ns: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            recency_bias_total_results: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            recency_bias_calculation_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Set persistent storage for embedding persistence
    /// Embeddings will be saved to this storage when added
    pub fn with_persistent_storage(
        mut self,
        storage: Arc<dyn crate::storage::traits::VectorStorage>,
    ) -> Self {
        self.persistent_storage = Some(storage);
        self
    }

    /// Set persistent storage (mutable version)
    pub fn set_persistent_storage(
        &mut self,
        storage: Arc<dyn crate::storage::traits::VectorStorage>,
    ) {
        self.persistent_storage = Some(storage);
    }

    /// Persist a vector to storage if configured
    async fn persist_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<()> {
        if let Some(storage) = &self.persistent_storage {
            storage
                .add_vector(vector, metadata.clone())
                .await
                .context(format!("Failed to persist embedding {}", metadata.id))?;
        }
        Ok(())
    }

    /// Load embeddings from persistent storage into the in-memory vector database
    /// Call this after creating the vectorizer to restore persisted embeddings
    pub async fn load_embeddings_from_storage(&self, session_id: &str) -> Result<usize> {
        let storage = match &self.persistent_storage {
            Some(s) => s,
            None => {
                debug!("No persistent storage configured, skipping embedding load");
                return Ok(0);
            }
        };

        // Check if session has any embeddings
        if !storage.has_session_embeddings(session_id).await {
            debug!("No embeddings found in storage for session {}", session_id);
            return Ok(0);
        }

        // Load vectors from storage
        let vectors = storage.get_session_vectors(session_id).await?;
        let count = vectors.len();

        if count == 0 {
            debug!("No vectors returned from storage for session {}", session_id);
            return Ok(0);
        }

        info!("Loading {} embeddings from storage for session {}", count, session_id);

        // Add vectors to in-memory vector database
        let mut loaded = 0;
        for (vector, metadata) in vectors {
            match self.vector_db.add_vector(vector, metadata.clone()) {
                Ok(_) => loaded += 1,
                Err(e) => {
                    warn!("Failed to load vector {} into memory: {}", metadata.id, e);
                }
            }
        }

        info!("Successfully loaded {} embeddings into memory for session {}", loaded, session_id);
        Ok(loaded)
    }

    /// Load all embeddings from persistent storage into the in-memory vector database
    /// Call this on startup to restore all persisted embeddings
    pub async fn load_all_embeddings_from_storage(&self) -> Result<usize> {
        let storage = match &self.persistent_storage {
            Some(s) => s,
            None => {
                debug!("No persistent storage configured, skipping embedding load");
                return Ok(0);
            }
        };

        // Load all vectors from storage
        let vectors = storage.get_all_vectors().await?;
        let count = vectors.len();

        if count == 0 {
            debug!("No vectors found in storage");
            return Ok(0);
        }

        info!("Loading {} embeddings from storage into memory", count);

        // Add vectors to in-memory vector database
        let mut loaded = 0;
        for (vector, metadata) in vectors {
            match self.vector_db.add_vector(vector, metadata.clone()) {
                Ok(_) => loaded += 1,
                Err(e) => {
                    warn!("Failed to load vector {} into memory: {}", metadata.id, e);
                }
            }
        }

        info!("Successfully loaded {} embeddings into memory", loaded);
        Ok(loaded)
    }

    /// Check if persistent storage has embeddings for a session
    pub async fn has_persisted_embeddings(&self, session_id: &str) -> bool {
        if let Some(storage) = &self.persistent_storage {
            storage.has_session_embeddings(session_id).await
        } else {
            false
        }
    }

    /// Get count of persisted embeddings for a session
    pub async fn count_persisted_embeddings(&self, session_id: &str) -> usize {
        if let Some(storage) = &self.persistent_storage {
            storage.count_session_embeddings(session_id).await
        } else {
            0
        }
    }

    /// Vectorize all content in a session
    /// # Errors
    ///
    /// Returns an error if vectorization fails or if there are issues with the vector database
    pub async fn vectorize_session(&self, session: &ActiveSession) -> Result<usize> {
        info!("Vectorizing session: {}", session.id());
        let mut vectorized_count = 0;

        // Vectorize context updates
        vectorized_count += self.vectorize_context_updates(session).await?;

        // Vectorize entity descriptions if enabled
        if self.config.enable_entity_vectorization {
            vectorized_count += self.vectorize_entities(session).await?;
        }

        info!(
            "Vectorized {} items for session {}",
            vectorized_count, session.id()
        );
        Ok(vectorized_count)
    }

    /// Vectorize only the most recent update (incremental vectorization)
    /// This is much more efficient than re-vectorizing the entire session
    pub async fn vectorize_latest_update(&self, session: &ActiveSession) -> Result<usize> {
        debug!("Vectorizing latest update for session: {}", session.id());

        // Get the most recent update from hot context (VecDeque uses back() not last())
        let latest_update = session.hot_context.back();

        // Extract update or return early if none
        let update = match latest_update {
            Some(u) => u,
            None => {
                debug!("No updates to vectorize for session {}", session.id());
                return Ok(0);
            }
        };

        let raw_text = Self::extract_text_from_update(&update);

        if !self.should_vectorize_text(&raw_text) {
            debug!("Skipping vectorization - text too short or empty");
            return Ok(0);
        }

        // Apply smart summarization if text is too long
        let prepared_text = self.prepare_text_for_vectorization(&raw_text);

        debug!(
            "Prepared text for vectorization: {} chars (original: {} chars)",
            prepared_text.len(),
            raw_text.len()
        );

        let content_type = Self::determine_content_type(&update);

        // Generate embedding for single update
        let embeddings = self.embedding_engine.encode_batch(vec![prepared_text.clone()]).await?;

        if let Some(embedding) = embeddings.into_iter().next() {
            let metadata = VectorMetadata::new(
                update.id.to_string(),
                prepared_text,
                session.id().to_string(),
                format!("{content_type:?}"),
            );

            // Add to in-memory DB first
            match self.vector_db.add_vector(embedding.clone(), metadata.clone()) {
                Ok(_) => {
                    // CRITICAL: Only mark as vectorized if persistence succeeds
                    if let Err(e) = self.persist_vector(embedding, metadata).await {
                         warn!("Failed to persist vector for update {}: {}. Will NOT mark as vectorized.", update.id, e);
                         // We don't remove from in-memory DB here as it's fine for current session, 
                         // but we don't mark as persisted so it retries next time.
                         Ok(0)
                    } else {
                        debug!("Successfully vectorized and persisted latest update {}", update.id);
                        session.vectorized_update_ids.insert(update.id);
                        Ok(1)
                    }
                }
                Err(e) => {
                    warn!("Failed to vectorize latest update (in-memory): {}", e);
                    Ok(0)
                }
            }
        } else {
            warn!("No embedding generated for latest update");
            Ok(0)
        }
    }

    /// Vectorize context updates from a session
    async fn vectorize_context_updates(&self, session: &ActiveSession) -> Result<usize> {
        // CRITICAL FIX: vectorized_update_ids is persisted to RocksDB but vector_db is in-memory.
        // After daemon restart, vectorized_update_ids may contain IDs that are no longer in vector_db.
        // We must verify EACH update_id actually exists in vector_db, not just check if session has any.
        let session_id_str = session.id().to_string();

        // Get the actual vectorized IDs from vector_db for this session
        let actual_vectorized: std::collections::HashSet<String> = self
            .vector_db
            .get_vectorized_update_ids(&session_id_str)
            .into_iter()
            .collect();

        // Remove stale IDs from session.vectorized_update_ids that don't exist in vector_db
        let stale_ids: Vec<uuid::Uuid> = session
            .vectorized_update_ids
            .iter()
            .filter(|id| !actual_vectorized.contains(&id.to_string()))
            .map(|id| *id)
            .collect();

        if !stale_ids.is_empty() {
            tracing::info!(
                "Removing {} stale vectorized_update_ids for session {} (not in vector_db)",
                stale_ids.len(),
                session.id()
            );
            for stale_id in stale_ids {
                session.vectorized_update_ids.remove(&stale_id);
            }
        }

        // Count only non-vectorized updates for threshold decision
        let non_vectorized_count = session
            .incremental_updates
            .iter()
            .filter(|u| !session.vectorized_update_ids.contains(&u.id))
            .count();

        // Use parallel processing for large sessions, sequential for small ones
        let (texts_to_embed, metadata_list) = if non_vectorized_count >= PARALLEL_PROCESSING_THRESHOLD {
            debug!(
                "Using parallel processing for {} updates (threshold: {})",
                non_vectorized_count, PARALLEL_PROCESSING_THRESHOLD
            );
            self.collect_updates_parallel(session)?
        } else {
            debug!("Using sequential processing for {} updates", non_vectorized_count);
            self.collect_updates_sequential(session)?
        };

        if texts_to_embed.is_empty() {
            debug!("No new texts to vectorize for session {}", session.id());
            return Ok(0);
        }

        // Generate embeddings in batches
        let embeddings = self.embedding_engine.encode_batch(texts_to_embed.clone()).await?;

        // Add to vector database and track successfully vectorized IDs
        let mut added_count = 0;
        for (embedding, metadata) in embeddings.into_iter().zip(metadata_list.iter()) {
            match self.vector_db.add_vector(embedding.clone(), metadata.clone()) {
                Ok(_) => {
                    // Only mark as vectorized if persistence succeeds
                    match self.persist_vector(embedding, metadata.clone()).await {
                        Ok(_) => {
                             added_count += 1;
                             // Mark update as vectorized
                             if let Ok(update_id) = Uuid::parse_str(&metadata.id) {
                                 session.vectorized_update_ids.insert(update_id);
                             }
                        }
                        Err(e) => {
                            warn!("Failed to persist vector {}: {}. Skipping metadata update.", metadata.id, e);
                        }
                    }
                }
                Err(e) => warn!("Failed to add vector to database: {}", e),
            }
        }

        debug!("Added {} context update vectors, marked as vectorized", added_count);
        Ok(added_count)
    }

    /// Vectorize entity descriptions
    async fn vectorize_entities(&self, session: &ActiveSession) -> Result<usize> {
        let entity_count = session.entity_graph.entities.len();

        // Use parallel processing for large entity graphs, sequential for small ones
        let (texts_to_embed, metadata_list) = if entity_count >= PARALLEL_PROCESSING_THRESHOLD {
            debug!(
                "Using parallel processing for {} entities (threshold: {})",
                entity_count, PARALLEL_PROCESSING_THRESHOLD
            );

            // Parallel entity processing
            let results: Vec<(String, VectorMetadata)> = session
                .entity_graph
                .entities
                .par_iter()
                .filter_map(|(entity_name, entity_data)| {
                    // Create a comprehensive description of the entity
                    let mut description_parts = Vec::new();

                    // Add entity name
                    description_parts.push(entity_name.clone());

                    // Add entity type if available
                    description_parts.push(format!("Type: {:?}", entity_data.entity_type));

                    // Add related entities context
                    let related: Vec<String> = session
                        .entity_graph
                        .get_entity_relationships(entity_name)
                        .into_iter()
                        .take(5)
                        .map(|(target, rel, _)| format!("{} {:?}", target, rel))
                        .collect();
                    if !related.is_empty() {
                        description_parts.push(format!("Related to: {}", related.join(", ")));
                    }

                    // Add mention context
                    if entity_data.mention_count > 0 {
                        description_parts
                            .push(format!("Mentioned {} times", entity_data.mention_count));
                    }

                    let entity_text = description_parts.join(". ");

                    if self.should_vectorize_text(&entity_text) {
                        let metadata = VectorMetadata::new(
                            format!("entity:{entity_name}"),
                            entity_text.clone(),
                            session.id().to_string(),
                            "EntityDescription".to_string(),
                        );
                        Some((entity_text, metadata))
                    } else {
                        None
                    }
                })
                .collect();

            results.into_iter().unzip()
        } else {
            debug!("Using sequential processing for {} entities", entity_count);

            // Sequential entity processing
            let mut texts_to_embed = Vec::new();
            let mut metadata_list = Vec::new();

            for (entity_name, entity_data) in &session.entity_graph.entities {
                // Create a comprehensive description of the entity
                let mut description_parts = Vec::new();

                // Add entity name
                description_parts.push(entity_name.clone());

                // Add entity type if available
                description_parts.push(format!("Type: {:?}", entity_data.entity_type));

                // Add related entities context
                let related: Vec<String> = session
                    .entity_graph
                    .get_entity_relationships(entity_name)
                    .into_iter()
                    .take(5)
                    .map(|(target, rel, _)| format!("{} {:?}", target, rel))
                    .collect();
                if !related.is_empty() {
                    description_parts.push(format!("Related to: {}", related.join(", ")));
                }

                // Add mention context
                if entity_data.mention_count > 0 {
                    description_parts
                        .push(format!("Mentioned {} times", entity_data.mention_count));
                }

                let entity_text = description_parts.join(". ");

                if self.should_vectorize_text(&entity_text) {
                    texts_to_embed.push(entity_text.clone());
                    metadata_list.push(VectorMetadata::new(
                        format!("entity:{entity_name}"),
                        entity_text,
                        session.id().to_string(),
                        "EntityDescription".to_string(),
                    ));
                }
            }

            (texts_to_embed, metadata_list)
        };

        if texts_to_embed.is_empty() {
            debug!("No entity texts to vectorize for session {}", session.id());
            return Ok(0);
        }

        // Generate embeddings
        let embeddings = self.embedding_engine.encode_batch(texts_to_embed.clone()).await?;

        // Add to vector database
        let mut added_count = 0;
        for (embedding, metadata) in embeddings.into_iter().zip(metadata_list) {
            match self.vector_db.add_vector(embedding.clone(), metadata.clone()) {
                Ok(_) => {
                    if let Err(e) = self.persist_vector(embedding, metadata).await {
                        warn!("Failed to persist entity vector: {}", e);
                    } else {
                        added_count += 1;
                    }
                }
                Err(e) => warn!("Failed to add entity vector to database: {}", e),
            }
        }

        debug!("Added {} entity vectors", added_count);
        Ok(added_count)
    }

    /// Perform semantic search across all vectorized content
    ///
    /// This is the main search method that consolidates all search parameters
    /// into a single SearchOptions struct, eliminating method proliferation.
    ///
    /// # Errors
    ///
    /// Returns an error if the query cannot be embedded or if the vector database search fails
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

        // Create a hash of search parameters for cache key
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

        // Perform actual search with optional filters
        let search_results = match (session_filter, date_range) {
            // Both session and date filters
            (Some(session_id), Some((start, end))) => {
                self.vector_db
                    .search_with_filter(&query_embedding, limit, |metadata| {
                        metadata.source == session_id.to_string()
                            && metadata.timestamp >= start
                            && metadata.timestamp <= end
                    })?
            }
            // Only session filter
            (Some(session_id), None) => {
                self.vector_db
                    .search_in_source(&query_embedding, limit, &session_id.to_string())?
            }
            // Only date filter
            (None, Some((start, end))) if self.config.enable_cross_session_search => self
                .vector_db
                .search_with_filter(&query_embedding, limit, |metadata| {
                    metadata.timestamp >= start && metadata.timestamp <= end
                })?,
            // No filters, cross-session enabled
            (None, None) if self.config.enable_cross_session_search => {
                self.vector_db.search(&query_embedding, limit)?
            }
            // No cross-session search enabled
            _ => return Ok(Vec::new()),
        };

        let results = self.process_search_results(search_results, Some(recency_bias))?;

        // Cache the results
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

    /// Perform semantic search within a specific set of sessions
    ///
    /// This optimized version performs a single vector database search across all
    /// sessions, then applies optional scoring adjustments (recency bias, etc.) in post-processing.
    ///
    /// This avoids the O(n²) complexity of searching each session separately.
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

        // Create a hash of search parameters for cache key
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

        // Check query cache first
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

        // Pre-calculate valid session strings for fast lookup
        let valid_sessions: std::collections::HashSet<String> = allowed_sessions
            .iter()
            .map(|id| id.to_string())
            .collect();

        // Perform single search with filter across all sessions
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

        // Cache the results
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

    /// Process raw vector search results into semantic results
    fn process_search_results(
        &self,
        search_results: Vec<crate::core::vector_db::SearchMatch>,
        recency_bias_override: Option<f32>,
    ) -> Result<Vec<SemanticSearchResult>> {
        let now = Utc::now();
        // Defense-in-depth: Clamp recency_bias to prevent negative values or extreme values
        // This provides a safety net even if validation is bypassed elsewhere
        let recency_bias = recency_bias_override
            .unwrap_or(self.config.recency_bias)
            .clamp(0.0, 10.0); // Prevent negative values and extreme values

        // Create score adjuster if recency bias is enabled
        let score_adjuster: Option<Box<dyn crate::core::scoring::ScoreAdjuster>> = if recency_bias > 0.0 {
            Some(Box::new(crate::core::scoring::TemporalDecayAdjuster::new(recency_bias, now)))
        } else {
            None
        };

        // Start timing for decay calculation metrics
        let decay_start = std::time::Instant::now();
        let mut decay_count = 0;

        // Convert to semantic search results
        let mut results = Vec::new();
        for result in search_results {
            let content_type = Self::parse_content_type(&result.metadata.content_type);
            let session_id = Uuid::parse_str(&result.metadata.source)
                .context("Invalid session ID in metadata")?;

            // Calculate combined score (similarity + importance + content type weight)
            let importance_weight = content_type.importance_weight();
            let base_score = result.similarity.mul_add(0.7, importance_weight * 0.3);

            // Apply score adjustment using the strategy (e.g., temporal decay)
            let combined_score = if let Some(ref adjuster) = score_adjuster {
                decay_count += 1;
                adjuster.adjust(base_score, &result.metadata)
            } else {
                // No adjustment, use base score directly
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

        // Emit decay calculation performance metrics
        if decay_count > 0 {
            let decay_duration_ns = decay_start.elapsed().as_nanos() as u64;

            // Update atomic counters - lock-free
            self.recency_bias_total_duration_ns.fetch_add(decay_duration_ns, std::sync::atomic::Ordering::Relaxed);
            self.recency_bias_total_results.fetch_add(decay_count, std::sync::atomic::Ordering::Relaxed);
            self.recency_bias_calculation_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                    // Replace if new result has higher score or newer timestamp
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

        let results: Vec<SemanticSearchResult> = dedup_map.into_values().collect();
        debug!(
            "Deduplicated {} results to {} unique",
            original_count,
            results.len()
        );

        // Sort by combined score (highest first)
        let mut results = results;
        results.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!("Found {} semantic search results", results.len());
        Ok(results)
    }

    /// Get query cache statistics
    pub fn get_query_cache_stats(&self) -> Option<crate::core::query_cache::QueryCacheStatsSnapshot> {
        match &self.query_cache {
            Some(cache) => Some(cache.get_stats()),
            None => None,
        }
    }

    /// Get query cache efficiency metrics
    pub fn get_cache_efficiency_metrics(&self) -> Option<HashMap<String, f32>> {
        match &self.query_cache {
            Some(cache) => Some(cache.get_efficiency_metrics()),
            None => None,
        }
    }

    /// Get recency bias performance metrics (lock-free)
    ///
    /// Returns performance metrics for temporal decay calculations when recency_bias > 0.
    /// Metrics include total time, result counts, and averages across all searches.
    ///
    /// # Returns
    ///
    /// - `Some(metrics)` - HashMap containing performance metrics
    /// - `None` - No recency bias metrics available (never used)
    ///
    /// # Metrics
    ///
    /// - `recency_bias.total_duration_ns` - Total time spent calculating decay (nanoseconds)
    /// - `recency_bias.total_results` - Total number of results processed
    /// - `recency_bias.calculation_count` - Number of search operations with recency bias
    /// - `recency_bias.avg_duration_ns` - Average duration per calculation
    /// - `recency_bias.avg_results_per_calculation` - Average results processed per calculation
    /// - `recency_bias.avg_time_per_result_ns` - Average time per result (nanoseconds)
    pub fn get_recency_bias_metrics(&self) -> Option<HashMap<String, f32>> {
        let calculation_count = self.recency_bias_calculation_count.load(std::sync::atomic::Ordering::Relaxed);

        if calculation_count == 0 {
            return None;
        }

        let total_duration_ns = self.recency_bias_total_duration_ns.load(std::sync::atomic::Ordering::Relaxed);
        let total_results = self.recency_bias_total_results.load(std::sync::atomic::Ordering::Relaxed);

        let mut metrics = HashMap::new();
        metrics.insert("recency_bias.total_duration_ns".to_string(), total_duration_ns as f32);
        metrics.insert("recency_bias.total_results".to_string(), total_results as f32);
        metrics.insert("recency_bias.calculation_count".to_string(), calculation_count as f32);
        metrics.insert("recency_bias.avg_duration_ns".to_string(), total_duration_ns as f32 / calculation_count as f32);
        metrics.insert("recency_bias.avg_results_per_calculation".to_string(), total_results as f32 / calculation_count as f32);

        if total_results > 0 {
            metrics.insert("recency_bias.avg_time_per_result_ns".to_string(), total_duration_ns as f32 / total_results as f32);
        } else {
            metrics.insert("recency_bias.avg_time_per_result_ns".to_string(), 0.0);
        }

        Some(metrics)
    }

    /// Clear query cache
    ///
    /// # Errors
    ///
    /// Returns an error if the cache clearing operation fails
    pub async fn clear_query_cache(&self) -> Result<()> {
        if let Some(ref cache) = self.query_cache {
            cache.clear()?;
            info!("Query cache cleared successfully");
        }
        Ok(())
    }

    /// Invalidate cache entries for a specific session
    /// This is more efficient than clearing the entire cache
    ///
    /// # Errors
    ///
    /// Returns an error if the cache invalidation operation fails
    pub async fn invalidate_session_cache(&self, session_id: Uuid) -> Result<()> {
        if let Some(ref cache) = self.query_cache {
            cache.invalidate_session(session_id)?;
            debug!("Invalidated cache entries for session {}", session_id);
        }
        Ok(())
    }

    /// Check if query caching is enabled
    pub const fn is_query_caching_enabled(&self) -> bool {
        self.query_cache.is_some()
    }

    /// Find related content across sessions
    /// Find related content within a session
    ///
    /// # Errors
    ///
    /// Returns an error if the semantic search fails or if query processing encounters issues
    pub async fn find_related_content(
        &self,
        session: &ActiveSession,
        topic: &str,
        limit: usize,
    ) -> Result<Vec<SemanticSearchResult>> {
        let results = self.semantic_search(topic, limit * 2, None, SearchOptions::default()).await?;

        // Filter out current session and return top results
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

    /// Get statistics about vectorized content
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

    /// Check if a session has been vectorized
    pub fn is_session_vectorized(&self, session_id: uuid::Uuid) -> bool {
        // Check if session has UPDATE embeddings (not just entities)
        // This ensures auto-vectorization runs even if only entities are vectorized
        self.vector_db
            .has_session_update_embeddings(&session_id.to_string())
    }

    /// Count embeddings for a specific session
    pub fn count_session_embeddings(&self, session_id: uuid::Uuid) -> usize {
        self.vector_db
            .count_session_embeddings(&session_id.to_string())
    }

    /// Compute semantic similarity between two text strings
    /// Returns a similarity score between 0.0 and 1.0
    pub async fn compute_text_similarity(&self, text1: &str, text2: &str) -> Result<f32> {
        let embedding1 = self.embedding_engine.encode_text(text1).await?;
        let embedding2 = self.embedding_engine.encode_text(text2).await?;

        // Calculate cosine similarity
        let dot_product: f32 = embedding1.iter().zip(embedding2.iter()).map(|(a, b)| a * b).sum();
        let norm1: f32 = embedding1.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm2: f32 = embedding2.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm1 == 0.0 || norm2 == 0.0 {
            return Ok(0.0);
        }

        Ok(dot_product / (norm1 * norm2))
    }

    /// Collect updates sequentially (for small sessions)
    /// Uses incremental_updates to ensure ALL updates are vectorized (including evicted ones)
    fn collect_updates_sequential(
        &self,
        session: &ActiveSession,
    ) -> Result<(Vec<String>, Vec<VectorMetadata>)> {
        let mut texts_to_embed = Vec::new();
        let mut metadata_list = Vec::new();

        // Process ALL incremental updates (hot + warm + cold tier)
        // Skip already vectorized updates for efficiency
        for update in session.incremental_updates.iter() {
            // Skip if already vectorized
            if session.vectorized_update_ids.contains(&update.id) {
                continue;
            }

            let raw_text = Self::extract_text_from_update(update);
            if self.should_vectorize_text(&raw_text) {
                let content_type = Self::determine_content_type(update);
                let prepared_text = self.prepare_text_for_vectorization(&raw_text);
                texts_to_embed.push(prepared_text.clone());
                metadata_list.push(VectorMetadata::new(
                    update.id.to_string(),
                    prepared_text,
                    session.id().to_string(),
                    format!("{content_type:?}"),
                ));
            }
        }

        Ok((texts_to_embed, metadata_list))
    }

    /// Collect updates in parallel (for large sessions)
    /// Uses incremental_updates to ensure ALL updates are vectorized (including evicted ones)
    fn collect_updates_parallel(
        &self,
        session: &ActiveSession,
    ) -> Result<(Vec<String>, Vec<VectorMetadata>)> {
        // Process ALL incremental updates in parallel
        // Skip already vectorized updates for efficiency
        let results: Vec<(String, VectorMetadata)> = session
            .incremental_updates
            .par_iter()
            .filter_map(|update| {
                // Skip if already vectorized
                if session.vectorized_update_ids.contains(&update.id) {
                    return None;
                }

                let raw_text = Self::extract_text_from_update(update);
                if self.should_vectorize_text(&raw_text) {
                    let content_type = Self::determine_content_type(update);
                    let prepared_text = self.prepare_text_for_vectorization(&raw_text);
                    let metadata = VectorMetadata::new(
                        update.id.to_string(),
                        prepared_text.clone(),
                        session.id().to_string(),
                        format!("{content_type:?}"),
                    );
                    Some((prepared_text, metadata))
                } else {
                    None
                }
            })
            .collect();

        // Split results into texts and metadata
        let mut texts_to_embed = Vec::with_capacity(results.len());
        let mut metadata_list = Vec::with_capacity(results.len());

        for (text, metadata) in results {
            texts_to_embed.push(text);
            metadata_list.push(metadata);
        }

        Ok((texts_to_embed, metadata_list))
    }

    /// Extract text content from a context update
    fn extract_text_from_update(update: &ContextUpdate) -> String {
        let mut text_parts = Vec::new();

        // ALWAYS include title (question) for all update types
        // For QA updates: Including the question is CRITICAL for semantic search
        // Users typically search with questions, not answers, so we need question text
        // in the embedding to match query embeddings effectively
        let title = update.content.title.clone();
        let description = update.content.description.clone();

        tracing::debug!("extract_text_from_update: title='{}', description='{}'",
            &title[..title.len().min(50)],
            &description[..description.len().min(50)]);

        text_parts.push(title);
        text_parts.push(description);

        // Add details
        for detail in &update.content.details {
            text_parts.push(detail.clone());
        }

        // Add examples
        for example in &update.content.examples {
            text_parts.push(example.clone());
        }

        // Add implications
        for implication in &update.content.implications {
            text_parts.push(implication.clone());
        }

        // Add code reference if available
        if let Some(code_ref) = &update.related_code {
            text_parts.push(code_ref.code_snippet.clone());
            text_parts.push(code_ref.file_path.clone());
        }

        text_parts.join(" ")
    }
    /// Determine content type from context update
    const fn determine_content_type(update: &ContextUpdate) -> ContentType {
        match &update.update_type {
            crate::core::context_update::UpdateType::QuestionAnswered => ContentType::UserMessage,
            crate::core::context_update::UpdateType::ProblemSolved => ContentType::ProblemSolution,
            crate::core::context_update::UpdateType::CodeChanged => ContentType::CodeSnippet,
            crate::core::context_update::UpdateType::DecisionMade => ContentType::DecisionPoint,
            crate::core::context_update::UpdateType::ConceptDefined
            | crate::core::context_update::UpdateType::RequirementAdded => {
                ContentType::UpdateContent
            }
        }
    }

    /// Check if text should be vectorized (with smart summarization for long texts)
    fn should_vectorize_text(&self, text: &str) -> bool {
        let len = text.trim().len();
        len >= self.config.min_text_length
        // Note: No upper limit check - long texts will be summarized
    }

    /// Prepare text for vectorization, applying smart summarization if needed
    fn prepare_text_for_vectorization(&self, text: &str) -> String {
        let len = text.trim().len();

        if len <= self.config.max_text_length {
            // Text is within limits, use as-is
            return text.to_string();
        }

        // Text is too long, apply smart summarization
        debug!(
            "Text length {} exceeds max {}, applying smart summarization",
            len, self.config.max_text_length
        );

        Self::extract_key_points(text, self.config.max_text_length)
    }

    /// Extract key points from long text using smart heuristics
    ///
    /// Strategy:
    /// 1. Preserve Title and Description (most important context)
    /// 2. Extract code snippets (technical details)
    /// 3. Extract bullet points and numbered lists (structured info)
    /// 4. Include beginning (context) and end (conclusion)
    /// 5. Preserve key phrases and technical terms
    fn extract_key_points(text: &str, max_length: usize) -> String {
        let mut parts = Vec::new();
        let mut current_length = 0;

        // Helper function to add part if it fits
        let add_part = |parts: &mut Vec<String>, current_len: &mut usize, part: &str| -> bool {
            let part_len = part.len();
            if *current_len + part_len + 3 <= max_length {  // +3 for " | "
                parts.push(part.to_string());
                *current_len += part_len + 3;
                true
            } else {
                false
            }
        };

        // 1. Always preserve Title and Description (highest priority)
        for line in text.lines() {
            if (line.starts_with("Title:") || line.starts_with("Description:"))
                && !add_part(&mut parts, &mut current_length, line) {
                    // Title/Description alone exceeds max_length - truncate it
                    let truncated = &line.chars().take(max_length - 20).collect::<String>();
                    parts.push(format!("{}...", truncated));
                    return parts.join(" | ");
                }
        }

        // 2. Extract code snippets (technical information is valuable)
        let code_blocks: Vec<&str> = text
            .split("Code:")
            .skip(1)  // Skip first part (before "Code:")
            .filter_map(|segment| {
                // Extract first line after "Code:"
                segment.split('|').next().map(|s| s.trim())
            })
            .collect();

        for code in code_blocks.iter().take(2) {  // Max 2 code snippets
            if !add_part(&mut parts, &mut current_length, &format!("Code: {}", code)) {
                break;
            }
        }

        // 3. Extract bullet points and key technical terms
        // Look for patterns like: "- item", "* item", "1. item", "Performance:", "Algorithm:"
        let mut key_lines = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('-')
                || trimmed.starts_with('*')
                || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
                || trimmed.contains("Performance:")
                || trimmed.contains("Algorithm:")
                || trimmed.contains("O(")  // Big-O notation
                || trimmed.contains("speedup")
                || trimmed.contains("optimization")
            {
                key_lines.push(trimmed);
            }
        }

        for line in key_lines.iter().take(5) {  // Max 5 key lines
            if !add_part(&mut parts, &mut current_length, line) {
                break;
            }
        }

        // 4. If we still have space, add beginning context
        if current_length < max_length * 3 / 4 {
            let intro_budget = (max_length - current_length).min(300);
            if intro_budget > 50 {
                let intro: String = text.chars().take(intro_budget).collect();
                if let Some(last_space) = intro.rfind(' ') {
                    let _ = add_part(&mut parts, &mut current_length, &format!("Context: {}...", &intro[..last_space]));
                }
            }
        }

        // 5. Add metadata about summarization
        if current_length < max_length - 50 {
            let _ = add_part(&mut parts, &mut current_length, &format!("[Summarized from {} chars]", text.len()));
        }

        if parts.is_empty() {
            // Fallback: just truncate at word boundary
            let truncated: String = text.chars().take(max_length - 20).collect();
            if let Some(last_space) = truncated.rfind(' ') {
                format!("{}... [Truncated]", &truncated[..last_space])
            } else {
                format!("{}... [Truncated]", truncated)
            }
        } else {
            parts.join(" | ")
        }
    }

    /// Parse content type from string
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
}
